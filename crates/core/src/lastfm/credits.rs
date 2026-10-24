//! Last.fm bios for the people a track credits beyond its album artist —
//! every name in its `artist` and `vocalist` tags — for the Now Playing tab.
//!
//! A tag value is looked up whole first, so a name that contains a joiner
//! ("Simon & Garfunkel") still matches; only when the whole value has no bio
//! is it split into names. A name whose bio is already on the tab is skipped:
//! the album artist's comes first, then the vocalists', so a singer credited
//! in both tags appears once, as a vocalist.
//!
//! Vocalist tags here are often romanized ("Kano") where the artist tag keeps
//! the original ("Tomoya feat. 鹿乃"), and Last.fm files Japanese artists under
//! the original. The sort tags pair the two spellings, so a romanized name is
//! looked up under its original first.

use super::{album_json, artist_info, str_of, ArtistInfo};
use crate::db::{has_cjk, Db};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Names shorter than this many characters too often belong to an unrelated
/// Last.fm artist of the same name, so they are only shown when the album
/// confirms them and their page is not a list of same-name artists.
const MIN_UNCONFIRMED_CHARS: usize = 5;

/// What joins two names in an `artist` or `vocalist` tag: `;` (how
/// multi-value tags are stored here), `；`, `/`, `／`, `×` and `、` anywhere;
/// feat., ft., featuring, vs, x, & and + between spaces; and a "feat."
/// with no space after it ("DJ Shimamura feat.美里").
static JOINER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s+(?:feat\.?|ft\.|featuring|vs\.?|x|&|\+)\s+|\s+(?:feat|ft)\.|\s*[;；/／×、]\s*",
    )
    .unwrap()
});

/// Last.fm keeps pages for misspelled names whose whole bio points at the
/// right page ("Incorrect tag for らっぷびと", "WRONG TAG for 溝口肇!", or
/// just the right page's link).
static REDIRECT_NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:this is an? )?(?:incorrect|wrong|misspelled) (?:artist )?(?:tag|name|spelling)|^\s*misspell|^\s*https?://\S+\s*$",
    )
    .unwrap()
});

/// A Last.fm page shared by several artists of one name: "There are
/// multiple artists with this name", a numbered list of them, or a pointer
/// to another one ("For the Japanese artist see たま"). An album match only
/// proves someone scrobbled the album under that name, so for a short name
/// such a page says nothing about which artist this is.
static DISAMBIGUATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*there (?:are|is|were)\b[^.:\n]{0,80}?\b(?:artists?|bands?|acts?|groups?|musicians?|projects?)\b|^\s*1(?:\)|\.\s)|^\s*for the [^.\n]{0,60}?\bsee\b",
    )
    .unwrap()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CreditRole {
    Artist,
    Vocalist,
}

#[derive(Debug, Clone, Serialize)]
pub struct Credit {
    pub role: CreditRole,
    /// The name the bio was found under: a whole tag value, one name split
    /// from it, or the original spelling of a romanized name.
    pub name: String,
    /// Its romanization from a sort tag or the vocalist tag, when known.
    pub sort: Option<String>,
    pub info: ArtistInfo,
}

/// Tags of the track being shown. `albumartist` and `albumartistsort` are what
/// the album artist section shows, which is the artist tag's pair when there
/// is no album artist.
#[derive(Debug, Default, Clone, Copy)]
pub struct CreditTags<'a> {
    pub albumartist: Option<&'a str>,
    pub albumartistsort: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub artistsort: Option<&'a str>,
    pub vocalist: Option<&'a str>,
    pub album: Option<&'a str>,
}

/// Bios for the track's artists, then its vocalists, each in tag order. A
/// failed lookup only drops that name, and without an API key there are none.
pub fn credits(db: &Db, api_key: &str, tags: CreditTags) -> Vec<Credit> {
    if api_key.is_empty() {
        return Vec::new();
    }
    let album = tags.album.map(str::trim).unwrap_or("");
    let album_of = |artist: &str| {
        album_json(db, api_key, artist, album).unwrap_or_else(|e| {
            log::warn!("last.fm album lookup for {artist:?} failed: {e}");
            None
        })
    };
    // Track artists on the album artist's copy of the album: a compilation
    // filed under "Various Artists" names its real artists there. Fetched
    // only once a short name needs confirming.
    let mut listed: Option<HashSet<String>> = None;
    resolve(
        tags,
        |name| {
            artist_info(db, api_key, name).unwrap_or_else(|e| {
                log::warn!("last.fm artist lookup for {name:?} failed: {e}");
                None
            })
        },
        |name| {
            let listed = listed.get_or_insert_with(|| {
                tags.albumartist
                    .and_then(|aa| album_of(aa.trim()))
                    .map(|a| track_artists(&a).iter().map(|n| norm(n)).collect())
                    .unwrap_or_default()
            });
            listed.contains(&norm(name)) || album_of(name).is_some()
        },
    )
}

/// The rules of [`credits`] without the network: `lookup` fetches a name's
/// Last.fm info, and `on_album` says whether Last.fm files this album under
/// that name or lists the name on its tracks.
fn resolve(
    tags: CreditTags,
    lookup: impl FnMut(&str) -> Option<ArtistInfo>,
    on_album: impl FnMut(&str) -> bool,
) -> Vec<Credit> {
    let mut r = Resolver {
        lookup,
        on_album,
        infos: HashMap::new(),
        albums: HashMap::new(),
        originals: HashMap::new(),
        sorts: HashMap::new(),
        shown_names: HashSet::new(),
        shown_urls: HashSet::new(),
    };
    for (value, sort) in [
        (tags.albumartist, tags.albumartistsort),
        (tags.artist, tags.artistsort),
    ] {
        for (name, sort, certain) in sort_pairs(value, sort) {
            r.originals
                .entry(sort_key(&sort))
                .or_insert_with(|| name.clone());
            if certain {
                r.sorts.entry(norm(&name)).or_insert(sort);
            }
        }
    }
    if let Some(aa) = tags.albumartist.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(info) = r.info(aa).filter(has_bio) {
            r.mark_shown(aa, &info);
        }
    }
    let vocalists = r.tag(tags.vocalist, CreditRole::Vocalist);
    let mut out = r.tag(tags.artist, CreditRole::Artist);
    out.extend(vocalists);
    out
}

enum Outcome {
    Shown,
    /// Its bio is already on the tab under another section.
    Covered,
    /// No bio, or a short name the album does not confirm.
    Missing,
}

struct Resolver<L, A> {
    lookup: L,
    on_album: A,
    /// Answers already fetched in this call, by normalized name: a name can
    /// turn up in both tags.
    infos: HashMap<String, Option<ArtistInfo>>,
    albums: HashMap<String, bool>,
    /// Original spelling by [`sort_key`] of a romanization the sort tags
    /// may pair it with, and the romanization to display by normalized
    /// original, for the pairings that are certain.
    originals: HashMap<String, String>,
    sorts: HashMap<String, String>,
    /// Normalized names and Last.fm URLs of the artists whose bio is shown.
    shown_names: HashSet<String>,
    shown_urls: HashSet<String>,
}

impl<L, A> Resolver<L, A>
where
    L: FnMut(&str) -> Option<ArtistInfo>,
    A: FnMut(&str) -> bool,
{
    /// Bios for one tag value: the whole value if Last.fm has a bio for it,
    /// otherwise each name split from it.
    fn tag(&mut self, value: Option<&str>, role: CreditRole) -> Vec<Credit> {
        let mut out = Vec::new();
        let Some(whole) = value.map(str::trim).filter(|s| !s.is_empty()) else {
            return out;
        };
        if let Outcome::Missing = self.credit(whole, role, &mut out) {
            for part in split_names(whole) {
                if part != whole {
                    self.credit(&part, role, &mut out);
                }
            }
        }
        out
    }

    /// One name from a tag, tried under its original spelling first when the
    /// sort tags pair it with one. The pairing counts only when the
    /// original's bio names the romanization: sort tags here often list the
    /// names in another order, or more names than the tag.
    fn credit(&mut self, name: &str, role: CreditRole, out: &mut Vec<Credit>) -> Outcome {
        if let Some(original) = self.originals.get(&sort_key(name)).cloned() {
            let confirmed = self
                .info(&original)
                .is_some_and(|info| has_bio(&info) && mentions(&info, name));
            if confirmed {
                if let found @ (Outcome::Shown | Outcome::Covered) =
                    self.find(&original, name, role, out)
                {
                    self.shown_names.insert(norm(name));
                    return found;
                }
            }
        }
        self.find(name, name, role, out)
    }

    /// Looks `name` up for a name the tag spells `spelled`. The length rule
    /// applies to the tag's spelling, since that is the name being credited.
    fn find(
        &mut self,
        name: &str,
        spelled: &str,
        role: CreditRole,
        out: &mut Vec<Credit>,
    ) -> Outcome {
        if self.shown_names.contains(&norm(name)) {
            return Outcome::Covered;
        }
        let Some(info) = self.info(name).filter(has_bio) else {
            return Outcome::Missing;
        };
        if self.is_shown(&info) {
            return Outcome::Covered;
        }
        if spelled.chars().count() < MIN_UNCONFIRMED_CHARS
            && (is_disambiguation(&info) || !self.on_album(name))
        {
            return Outcome::Missing;
        }
        self.mark_shown(name, &info);
        let sort = match self.sorts.get(&norm(name)) {
            Some(sort) => Some(sort.clone()),
            None => (name != spelled).then(|| spelled.to_string()),
        };
        out.push(Credit {
            role,
            name: name.to_string(),
            sort,
            info,
        });
        Outcome::Shown
    }

    fn info(&mut self, name: &str) -> Option<ArtistInfo> {
        let lookup = &mut self.lookup;
        self.infos
            .entry(norm(name))
            .or_insert_with(|| lookup(name))
            .clone()
    }

    fn on_album(&mut self, name: &str) -> bool {
        let on_album = &mut self.on_album;
        *self
            .albums
            .entry(norm(name))
            .or_insert_with(|| on_album(name))
    }

    fn is_shown(&self, info: &ArtistInfo) -> bool {
        info.url
            .as_ref()
            .is_some_and(|u| self.shown_urls.contains(&u.to_lowercase()))
            || self.shown_names.contains(&norm(&info.name))
    }

    fn mark_shown(&mut self, name: &str, info: &ArtistInfo) {
        self.shown_names.insert(norm(name));
        self.shown_names.insert(norm(&info.name));
        if let Some(url) = &info.url {
            self.shown_urls.insert(url.to_lowercase());
        }
    }
}

fn has_bio(info: &ArtistInfo) -> bool {
    [&info.bio_full, &info.bio_summary]
        .into_iter()
        .flatten()
        .any(|b| !b.is_empty() && !REDIRECT_NOTE.is_match(b))
}

fn is_disambiguation(info: &ArtistInfo) -> bool {
    [&info.bio_full, &info.bio_summary]
        .into_iter()
        .flatten()
        .any(|b| DISAMBIGUATION.is_match(b))
}

/// Whether a bio names `romaji` word for word, in any order ("name: Mao
/// Komiya" names "Komiya, Mao").
fn mentions(info: &ArtistInfo, romaji: &str) -> bool {
    fn words(s: &str) -> HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    }
    let wanted = words(romaji);
    let bio: HashSet<String> = [&info.bio_full, &info.bio_summary]
        .into_iter()
        .flatten()
        .flat_map(|b| words(b))
        .collect();
    !wanted.is_empty() && wanted.is_subset(&bio)
}

/// (original, romanization, certain) pairs from a tag and its sort tag, for
/// the names that need romanizing. Names pair by position when both split
/// into as many names, which is certain unless the sort tag reorders them; a
/// lone name pairs uncertainly with every name of a longer sort tag
/// ("綾倉盟" with "VALLEYSTONE; Ayakura Mei").
fn sort_pairs(value: Option<&str>, sort: Option<&str>) -> Vec<(String, String, bool)> {
    let value = value.map(str::trim).filter(|s| !s.is_empty());
    let sort = sort.map(str::trim).filter(|s| !s.is_empty());
    let (Some(value), Some(sort)) = (value, sort) else {
        return Vec::new();
    };
    let (names, sorts) = (split_names(value), split_names(sort));
    let named: HashSet<String> = names.iter().map(|n| norm(n)).collect();
    let mut pairs = Vec::new();
    if names.len() == sorts.len() {
        // A romanization that is another of the tag's names ("ココ/ARM"
        // sorted "ARM; Koko") means the order differs.
        let reordered = names
            .iter()
            .zip(&sorts)
            .any(|(n, s)| norm(n) != norm(s) && named.contains(&norm(s)));
        pairs.push((value.to_string(), sort.to_string(), !reordered));
        if names.len() > 1 {
            pairs.extend(
                names
                    .into_iter()
                    .zip(sorts)
                    .map(|(n, s)| (n, s, !reordered)),
            );
        }
    } else if let [name] = names.as_slice() {
        pairs.extend(sorts.iter().map(|s| (name.clone(), s.clone(), false)));
    }
    pairs.retain(|(n, s, _)| has_cjk(n) && !named.contains(&norm(s)));
    pairs
}

/// [`norm`], also folding "Komiya, Mao" (sort-tag order) into "komiya mao".
fn sort_key(s: &str) -> String {
    norm(&s.replace(',', " "))
}

/// The names in a tag value: parenthesized notes dropped ("Mai (SynthV)",
/// "uno (vocals: …)"), then split at each joiner.
fn split_names(value: &str) -> Vec<String> {
    let mut bare = String::with_capacity(value.len());
    let mut depth = 0usize;
    for c in value.chars() {
        match c {
            '(' | '（' => depth += 1,
            ')' | '）' => depth = depth.saturating_sub(1),
            _ if depth == 0 => bare.push(c),
            _ => {}
        }
    }
    JOINER
        .split(&bare)
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Artist names on the tracks of an album.getInfo `album` object. Last.fm
/// sends a lone track as an object instead of a one-element array.
fn track_artists(album: &Value) -> Vec<String> {
    let tracks = &album["tracks"]["track"];
    tracks
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(std::slice::from_ref(tracks))
        .iter()
        .filter_map(|t| str_of(&t["artist"]["name"]))
        .collect()
}

fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artist(name: &str, bio: bool) -> ArtistInfo {
        ArtistInfo {
            name: name.to_string(),
            bio_summary: bio.then(|| format!("{name} is an artist.")),
            bio_full: bio.then(|| format!("{name} is an artist.")),
            listeners: None,
            playcount: None,
            tags: Vec::new(),
            similar: Vec::new(),
            url: Some(format!("https://www.last.fm/music/{name}")),
        }
    }

    /// Runs [`resolve`] against a fake Last.fm that knows `bios` (with a bio)
    /// and `pages` (without one), and files the album under `on_album`.
    /// Returns (role, name) pairs and every name that was looked up.
    fn run(
        tags: CreditTags,
        bios: &[&str],
        pages: &[&str],
        on_album: &[&str],
    ) -> (Vec<(CreditRole, String)>, Vec<String>) {
        let mut asked = Vec::new();
        let out = resolve(
            tags,
            |name| {
                asked.push(name.to_string());
                if bios.contains(&name) {
                    Some(artist(name, true))
                } else if pages.contains(&name) {
                    Some(artist(name, false))
                } else {
                    None
                }
            },
            |name| on_album.contains(&name),
        );
        let out = out.into_iter().map(|c| (c.role, c.name)).collect();
        (out, asked)
    }

    use CreditRole::{Artist, Vocalist};

    #[test]
    fn artist_matching_the_shown_album_artist_is_hidden() {
        let tags = CreditTags {
            albumartist: Some("Demetori"),
            artist: Some("demetori"),
            ..Default::default()
        };
        assert!(run(tags, &["Demetori"], &[], &[]).0.is_empty());
    }

    #[test]
    fn artist_resolving_to_the_album_artists_page_is_hidden() {
        let tags = CreditTags {
            albumartist: Some("上海アリス幻樂団"),
            artist: Some("Team Shanghai Alice"),
            ..Default::default()
        };
        let out = resolve(tags, |_| Some(artist("上海アリス幻樂団", true)), |_| false);
        assert!(out.is_empty());
    }

    #[test]
    fn whole_value_is_tried_before_its_names() {
        let tags = CreditTags {
            albumartist: Some("Various Artists"),
            artist: Some("Simon & Garfunkel"),
            ..Default::default()
        };
        let (out, asked) = run(tags, &["Simon & Garfunkel", "Simon"], &[], &[]);
        assert_eq!(out, [(Artist, "Simon & Garfunkel".to_string())]);
        assert!(!asked.contains(&"Simon".to_string()));
    }

    #[test]
    fn joined_names_are_split_when_the_whole_value_has_no_bio() {
        let tags = CreditTags {
            albumartist: Some("IOSYS"),
            artist: Some("IOSYS feat. Nanahira"),
            ..Default::default()
        };
        let (out, _) = run(tags, &["IOSYS", "Nanahira"], &["IOSYS feat. Nanahira"], &[]);
        assert_eq!(out, [(Artist, "Nanahira".to_string())]);
    }

    #[test]
    fn album_artist_without_a_bio_hides_nothing() {
        let tags = CreditTags {
            albumartist: Some("Demetori & DARK PHOENiX"),
            artist: Some("Demetori & DARK PHOENiX"),
            ..Default::default()
        };
        let (out, _) = run(
            tags,
            &["Demetori", "DARK PHOENiX"],
            &["Demetori & DARK PHOENiX"],
            &[],
        );
        assert_eq!(
            out,
            [
                (Artist, "Demetori".to_string()),
                (Artist, "DARK PHOENiX".to_string())
            ]
        );
    }

    #[test]
    fn a_vocalist_is_shown_once_as_a_vocalist_and_last() {
        let tags = CreditTags {
            albumartist: Some("Camellia"),
            artist: Some("Nanahira; Camellia; Frums"),
            vocalist: Some("Nanahira"),
            ..Default::default()
        };
        let (out, _) = run(tags, &["Camellia", "Nanahira", "Frums"], &[], &[]);
        assert_eq!(
            out,
            [
                (Artist, "Frums".to_string()),
                (Vocalist, "Nanahira".to_string())
            ]
        );
    }

    #[test]
    fn vocalist_matching_the_album_artist_is_hidden() {
        let tags = CreditTags {
            albumartist: Some("Nanahira"),
            artist: Some("Nanahira"),
            vocalist: Some("Nanahira"),
            ..Default::default()
        };
        assert!(run(tags, &["Nanahira"], &[], &[]).0.is_empty());
    }

    #[test]
    fn short_names_are_shown_only_when_the_album_confirms_them() {
        let tags = CreditTags {
            albumartist: Some("Various Artists"),
            artist: Some("ak; 藤宮ゆき"),
            vocalist: Some("miko"),
            album: Some("Some Album"),
            ..Default::default()
        };
        let bios = ["ak", "藤宮ゆき", "miko"];
        assert!(run(tags, &bios, &[], &[]).0.is_empty());
        let (out, _) = run(tags, &bios, &[], &["ak", "miko"]);
        assert_eq!(
            out,
            [(Artist, "ak".to_string()), (Vocalist, "miko".to_string())]
        );
    }

    /// A fake Last.fm whose bio for each name is the given text.
    fn bios<'a>(pages: &'a [(&'a str, &'a str)]) -> impl FnMut(&str) -> Option<ArtistInfo> + 'a {
        |name| {
            pages.iter().find(|(n, _)| *n == name).map(|(n, bio)| {
                let mut info = artist(n, true);
                info.bio_full = Some(bio.to_string());
                info.bio_summary = Some(bio.to_string());
                info
            })
        }
    }

    #[test]
    fn a_romanized_vocalist_is_found_under_the_artist_tags_spelling() {
        let tags = CreditTags {
            albumartist: Some("Tomoya"),
            artist: Some("Tomoya feat. 小宮真央"),
            artistsort: Some("Tomoya feat. Komiya, Mao"),
            vocalist: Some("Komiya Mao"),
            ..Default::default()
        };
        let pages = [
            ("Tomoya", "Tomoya is an arranger."),
            ("小宮真央", "name: Mao Komiya circle: Asterisk"),
            ("Komiya Mao", "Incorrect tag for 小宮真央"),
        ];
        let out = resolve(tags, bios(&pages), |_| false);
        let out: Vec<_> = out
            .iter()
            .map(|c| (c.role, c.name.as_str(), c.sort.as_deref()))
            .collect();
        // Shown once, as a vocalist, though the original is under 5
        // characters: the vocalist tag spells it with 10.
        assert_eq!(out, [(Vocalist, "小宮真央", Some("Komiya, Mao"))]);
    }

    #[test]
    fn an_uncertain_pairing_needs_the_bio_to_name_the_romanization() {
        // The sort tag lists more names than the artist tag: 綾倉盟 could be
        // either, and only her bio says which.
        let tags = CreditTags {
            albumartist: Some("Pizuya's Cell"),
            artist: Some("綾倉盟"),
            artistsort: Some("VALLEYSTONE; Ayakura Mei"),
            vocalist: Some("Ayakura Mei"),
            ..Default::default()
        };
        let pages = [("綾倉盟", "Mei Ayakura (綾倉盟, Ayakura Mei) is a vocalist.")];
        let out = resolve(tags, bios(&pages), |_| false);
        let out: Vec<_> = out
            .iter()
            .map(|c| (c.role, c.name.as_str(), c.sort.as_deref()))
            .collect();
        assert_eq!(out, [(Vocalist, "綾倉盟", Some("Ayakura Mei"))]);

        // 柊秀雪 is the arranger, so his bio does not name the vocalist and he
        // stays an artist (shown since the album confirms his short name).
        let tags = CreditTags {
            albumartist: Some("Forest306"),
            artist: Some("柊秀雪"),
            artistsort: Some("Hiiragi Shuusetsu; Atori"),
            vocalist: Some("Atori"),
            ..Default::default()
        };
        let pages = [
            ("柊秀雪", "Hiiragi Shuusetsu is an arranger."),
            ("Atori", "Atori is a doujin singer."),
        ];
        let out = resolve(tags, bios(&pages), |name| name == "柊秀雪");
        let out: Vec<_> = out
            .iter()
            .map(|c| (c.role, c.name.as_str(), c.sort.as_deref()))
            .collect();
        assert_eq!(out, [(Artist, "柊秀雪", None), (Vocalist, "Atori", None)]);
    }

    #[test]
    fn an_album_match_does_not_rescue_a_short_name_with_a_shared_page() {
        let tags = CreditTags {
            albumartist: Some("Elemental Records"),
            artist: Some("ARU; ほたる; Stack"),
            album: Some("Sound Spectrum"),
            ..Default::default()
        };
        let pages = [
            ("ARU", "There are multiple artists called ARU."),
            (
                "ほたる",
                "ほたる (Hotaru) was the prime vocalist of the circle Halozy.",
            ),
            (
                "Stack",
                "There are three different Acts called “Stack”: 1) German Hardcore",
            ),
        ];
        let out = resolve(tags, bios(&pages), |_| true);
        let out: Vec<_> = out.iter().map(|c| c.name.as_str()).collect();
        // Five characters or more needs no confirmation, so Stack's shared
        // page is still shown.
        assert_eq!(out, ["ほたる", "Stack"]);
    }

    #[test]
    fn recognizes_lastfm_disambiguation_pages() {
        let bio = |text: &str| {
            let mut info = artist("x", true);
            info.bio_full = Some(text.to_string());
            info.bio_summary = None;
            info
        };
        // Pages seen for short names in this library.
        for text in [
            "There are at least fifteen (15) bands or artists named 'Sin'. Some of",
            "There is more than one artist with this name:  1) Noah Carter",
            "There are multiple artists called ARU.",
            "There are several artists who go by the name of Lego:  1) Lego is a",
            "There are at least seven distinct artists/groups working under the name",
            "There are multiple artists with this name.  1) The Finnish Europop band",
            "For the Japanese artist see たま\n\nBand that fuses West African",
            "1) Japanese arranger, DJ, composer. Circle: DiGiTAL WiNG  2) Katsu",
        ] {
            assert!(is_disambiguation(&bio(text)), "{text}");
        }
        for text in [
            "ほたる (Hotaru) was the prime vocalist of the circle Halozy until",
            "name: Koko circle: Amnesia www: http://www.geocities.jp/nyokikoko/",
            "Misato (美里), also known as Misty, is a Japanese female vocalist",
            "Known for her soft and girly voice, 鹿乃 (read: kano) is a singer",
            "La historia de Caló va mas allá de 1990",
            "There were no releases before 2010, when the circle formed.",
        ] {
            assert!(!is_disambiguation(&bio(text)), "{text}");
        }
    }

    #[test]
    fn a_reordered_sort_tag_pairs_nothing() {
        assert!(sort_pairs(Some("ココ/ARM"), Some("ARM; Koko"))
            .iter()
            .all(|(_, _, certain)| !certain));
        assert!(sort_pairs(Some("Un3h; noaon"), Some("noaon; Un3h")).is_empty());
    }

    #[test]
    fn split_artist_names_carry_their_romanization() {
        let tags = CreditTags {
            albumartist: Some("セブンスヘブンMAXION"),
            artist: Some("ビートまりお; myu314"),
            artistsort: Some("Beatmario; myu314"),
            ..Default::default()
        };
        let out = resolve(
            tags,
            |name| {
                ["ビートまりお", "myu314"]
                    .contains(&name)
                    .then(|| artist(name, true))
            },
            |_| false,
        );
        let out: Vec<_> = out
            .iter()
            .map(|c| (c.name.as_str(), c.sort.as_deref()))
            .collect();
        assert_eq!(out, [("ビートまりお", Some("Beatmario")), ("myu314", None)]);
    }

    #[test]
    fn redirect_notes_are_not_bios() {
        let mut note = artist("rapbit", true);
        note.bio_full = Some("Incorrect tag for らっぷびと".to_string());
        note.bio_summary = note.bio_full.clone();
        assert!(!has_bio(&note));
        note.bio_full = Some("WRONG TAG for 溝口肇!".to_string());
        note.bio_summary = None;
        assert!(!has_bio(&note));
        note.bio_full = Some("http://www.last.fm/music/%E9%BC%93%E5%A4%AA%E8%9D%8B".to_string());
        assert!(!has_bio(&note));
        assert!(has_bio(&artist("らっぷびと", true)));
    }

    #[test]
    fn splits_at_joiners_and_drops_notes() {
        let cases: [(&str, &[&str]); 9] = [
            ("Escarmew;和泉幸奇", &["Escarmew", "和泉幸奇"]),
            ("AO; ｙｔｒ; Matsu", &["AO", "ｙｔｒ", "Matsu"]),
            ("DJ Shimamura feat.美里", &["DJ Shimamura", "美里"]),
            ("t+pazolite feat. 鈴木ななこ", &["t+pazolite", "鈴木ななこ"]),
            (
                "Tracy vs. Astronomical feat.KUMI(ヲタみん)",
                &["Tracy", "Astronomical", "KUMI"],
            ),
            (
                "あやぽんず＊ x あよ X ビートまりお",
                &["あやぽんず＊", "あよ", "ビートまりお"],
            ),
            ("A-One×豚乙女", &["A-One", "豚乙女"]),
            ("uno (vocals: あいかぴん, ちよこ, 高井さん)", &["uno"]),
            ("COOL&CREATE", &["COOL&CREATE"]),
        ];
        for (value, names) in cases {
            assert_eq!(split_names(value), names, "{value}");
        }
        assert_eq!(
            split_names("Camellia / タカナシトリコ"),
            ["Camellia", "タカナシトリコ"]
        );
        assert_eq!(
            split_names("大瀬良あい/BlackY/ココ"),
            ["大瀬良あい", "BlackY", "ココ"]
        );
        assert_eq!(split_names("void (vocal: らんこ (豚乙女))"), ["void"]);
        assert_eq!(split_names("noa（discordia）"), ["noa"]);
    }

    #[test]
    fn reads_track_artists_from_arrays_and_singletons() {
        let many = json!({"tracks": {"track": [
            {"name": "A", "artist": {"name": "ak"}},
            {"name": "B", "artist": {"name": "Cryu"}}
        ]}});
        assert_eq!(track_artists(&many), ["ak", "Cryu"]);
        let one = json!({"tracks": {"track": {"name": "A", "artist": {"name": "ak"}}}});
        assert_eq!(track_artists(&one), ["ak"]);
        assert!(track_artists(&json!({})).is_empty());
    }
}
