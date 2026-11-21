//! Profiles for the people a track credits beyond its album artist — every
//! name in its `artist` and `vocalist` tags — for the Now Playing tab.
//!
//! A tag value is looked up whole first, so a name that contains a joiner
//! ("Simon & Garfunkel") still matches; only when the whole value has no
//! profile is it split into names. A name whose profile is already on the tab
//! is skipped: the album artist's comes first, then the vocalists', so a
//! singer credited in both tags appears once, as a vocalist.
//!
//! Vocalist tags here are often romanized ("Kano") where the artist tag keeps
//! the original ("Tomoya feat. 鹿乃"), and the sites file Japanese artists
//! under the original. The sort tags pair the two spellings, so a romanized
//! name is looked up under its original first.

use super::ArtistInfo;
use crate::db::has_cjk;
use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Names shorter than this many characters too often belong to an unrelated
/// artist of the same name, so a site's entry for one is only taken when the
/// site confirms it (see `profile::Hints::trusted`).
pub(crate) const MIN_UNCONFIRMED_CHARS: usize = 5;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CreditRole {
    Artist,
    Vocalist,
}

#[derive(Debug, Clone, Serialize)]
pub struct Credit {
    pub role: CreditRole,
    /// The name the profile was found under: a whole tag value, one name
    /// split from it, or the original spelling of a romanized name.
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
    pub title: Option<&'a str>,
}

/// Whether a name spelled this way may be taken on a site's lone entry.
pub(crate) fn distinctive(spelled: &str) -> bool {
    spelled.chars().count() >= MIN_UNCONFIRMED_CHARS
}

/// Profiles for the track's artists, then its vocalists, each in tag order.
/// `lookup` fetches a name's info; its second argument says whether the name
/// as the tag spells it is distinctive enough to need no confirmation.
pub(crate) fn resolve(
    tags: CreditTags,
    lookup: impl FnMut(&str, bool) -> Option<ArtistInfo>,
) -> Vec<Credit> {
    let mut r = Resolver {
        lookup,
        infos: HashMap::new(),
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
        if let Some(info) = r.info(aa, distinctive(aa)).filter(has_bio) {
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
    /// Its profile is already on the tab under another section.
    Covered,
    /// No profile, or a short name no site confirms.
    Missing,
}

struct Resolver<L> {
    lookup: L,
    /// Answers already fetched in this call, by normalized name and whether
    /// it was asked for as distinctive: a name can turn up in both tags.
    infos: HashMap<(String, bool), Option<ArtistInfo>>,
    /// Original spelling by [`sort_key`] of a romanization the sort tags
    /// may pair it with, and the romanization to display by normalized
    /// original, for the pairings that are certain.
    originals: HashMap<String, String>,
    sorts: HashMap<String, String>,
    /// Normalized names and profile pages of the artists whose profile is
    /// shown.
    shown_names: HashSet<String>,
    shown_urls: HashSet<String>,
}

impl<L> Resolver<L>
where
    L: FnMut(&str, bool) -> Option<ArtistInfo>,
{
    /// Profiles for one tag value: the whole value if a site has one for it,
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
    /// original's profile names the romanization: sort tags here often list
    /// the names in another order, or more names than the tag.
    fn credit(&mut self, name: &str, role: CreditRole, out: &mut Vec<Credit>) -> Outcome {
        if let Some(original) = self.originals.get(&sort_key(name)).cloned() {
            let confirmed = self
                .info(&original, distinctive(name))
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

    /// Looks `name` up for a name the tag spells `spelled`. Whether the name
    /// is distinctive goes by the tag's spelling, since that is the name
    /// being credited.
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
        let Some(info) = self.info(name, distinctive(spelled)).filter(has_bio) else {
            return Outcome::Missing;
        };
        if self.is_shown(&info) {
            return Outcome::Covered;
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

    fn info(&mut self, name: &str, distinctive: bool) -> Option<ArtistInfo> {
        let lookup = &mut self.lookup;
        self.infos
            .entry((norm(name), distinctive))
            .or_insert_with(|| lookup(name, distinctive))
            .clone()
    }

    fn is_shown(&self, info: &ArtistInfo) -> bool {
        info.bio
            .as_ref()
            .is_some_and(|bio| self.shown_urls.contains(&bio.url.to_lowercase()))
            || self.shown_names.contains(&norm(&info.name))
    }

    fn mark_shown(&mut self, name: &str, info: &ArtistInfo) {
        self.shown_names.insert(norm(name));
        self.shown_names.insert(norm(&info.name));
        if let Some(bio) = &info.bio {
            self.shown_urls.insert(bio.url.to_lowercase());
        }
    }
}

fn has_bio(info: &ArtistInfo) -> bool {
    info.bio.is_some()
}

/// Whether a profile names `romaji` word for word, in any order ("name: Mao
/// Komiya" names "Komiya, Mao").
fn mentions(info: &ArtistInfo, romaji: &str) -> bool {
    fn words(s: &str) -> HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    }
    let wanted = words(romaji);
    let bio = info
        .bio
        .as_ref()
        .map(|bio| words(&bio.text))
        .unwrap_or_default();
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

fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artistinfo::profile::{Profile, Source};

    fn artist_with(name: &str, bio: Option<&str>) -> ArtistInfo {
        ArtistInfo {
            name: name.to_string(),
            bio: bio.map(|text| Profile {
                source: Source::TouhouDb,
                text: text.to_string(),
                url: format!("https://touhoudb.com/Ar/{name}"),
            }),
            listeners: None,
            playcount: None,
            tags: Vec::new(),
            similar: Vec::new(),
        }
    }

    fn artist(name: &str) -> ArtistInfo {
        artist_with(name, Some(&format!("{name} is an artist.")))
    }

    /// Runs [`resolve`] against fake sites that have a profile for `bios`,
    /// and confirm the names in `confirmed` (a name they cannot confirm is
    /// only found when it is distinctive). Returns (role, name) pairs and
    /// every name that was looked up.
    fn run(
        tags: CreditTags,
        bios: &[&str],
        confirmed: &[&str],
    ) -> (Vec<(CreditRole, String)>, Vec<String>) {
        let mut asked = Vec::new();
        let out = resolve(tags, |name, distinctive| {
            asked.push(name.to_string());
            (bios.contains(&name) && (distinctive || confirmed.contains(&name)))
                .then(|| artist(name))
        });
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
        assert!(run(tags, &["Demetori"], &[]).0.is_empty());
    }

    #[test]
    fn artist_resolving_to_the_album_artists_page_is_hidden() {
        let tags = CreditTags {
            albumartist: Some("上海アリス幻樂団"),
            artist: Some("Team Shanghai Alice"),
            ..Default::default()
        };
        // Both names lead to one page on the site.
        let out = resolve(tags, |_, _| Some(artist("上海アリス幻樂団")));
        assert!(out.is_empty());
    }

    #[test]
    fn whole_value_is_tried_before_its_names() {
        let tags = CreditTags {
            albumartist: Some("Various Artists"),
            artist: Some("Simon & Garfunkel"),
            ..Default::default()
        };
        let (out, asked) = run(tags, &["Simon & Garfunkel", "Simon"], &[]);
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
        let (out, _) = run(tags, &["IOSYS", "Nanahira"], &[]);
        assert_eq!(out, [(Artist, "Nanahira".to_string())]);
    }

    #[test]
    fn album_artist_without_a_bio_hides_nothing() {
        let tags = CreditTags {
            albumartist: Some("Demetori & DARK PHOENiX"),
            artist: Some("Demetori & DARK PHOENiX"),
            ..Default::default()
        };
        let (out, _) = run(tags, &["Demetori", "DARK PHOENiX"], &[]);
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
        let (out, _) = run(tags, &["Camellia", "Nanahira", "Frums"], &[]);
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
        assert!(run(tags, &["Nanahira"], &[]).0.is_empty());
    }

    #[test]
    fn short_names_are_shown_only_when_a_site_confirms_them() {
        let tags = CreditTags {
            albumartist: Some("Various Artists"),
            artist: Some("ak; 藤宮ゆき"),
            vocalist: Some("miko"),
            album: Some("Some Album"),
            ..Default::default()
        };
        let bios = ["ak", "藤宮ゆき", "miko"];
        assert!(run(tags, &bios, &[]).0.is_empty());
        let (out, _) = run(tags, &bios, &["ak", "miko"]);
        assert_eq!(
            out,
            [(Artist, "ak".to_string()), (Vocalist, "miko".to_string())]
        );
    }

    /// Fake sites whose profile for each name is the given text.
    fn bios<'a>(
        pages: &'a [(&'a str, &'a str)],
    ) -> impl FnMut(&str, bool) -> Option<ArtistInfo> + 'a {
        |name, _| {
            pages
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(n, bio)| artist_with(n, Some(bio)))
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
            (
                "小宮真央",
                "Mao Komiya is a vocalist of the circle Asterisk.",
            ),
        ];
        // The original is under 5 characters, but the vocalist tag spells the
        // name with 10: it is asked for as distinctive.
        let mut asked = Vec::new();
        let mut pages = bios(&pages);
        let out = resolve(tags, |name, distinctive| {
            asked.push((name.to_string(), distinctive));
            pages(name, distinctive)
        });
        let out: Vec<_> = out
            .iter()
            .map(|c| (c.role, c.name.as_str(), c.sort.as_deref()))
            .collect();
        // Shown once, as a vocalist.
        assert_eq!(out, [(Vocalist, "小宮真央", Some("Komiya, Mao"))]);
        assert!(asked.contains(&("小宮真央".to_string(), true)));
    }

    #[test]
    fn an_uncertain_pairing_needs_the_bio_to_name_the_romanization() {
        // The sort tag lists more names than the artist tag: 綾倉盟 could be
        // either, and only her profile says which.
        let tags = CreditTags {
            albumartist: Some("Pizuya's Cell"),
            artist: Some("綾倉盟"),
            artistsort: Some("VALLEYSTONE; Ayakura Mei"),
            vocalist: Some("Ayakura Mei"),
            ..Default::default()
        };
        let pages = [("綾倉盟", "Mei Ayakura (綾倉盟, Ayakura Mei) is a vocalist.")];
        let out = resolve(tags, bios(&pages));
        let out: Vec<_> = out
            .iter()
            .map(|c| (c.role, c.name.as_str(), c.sort.as_deref()))
            .collect();
        assert_eq!(out, [(Vocalist, "綾倉盟", Some("Ayakura Mei"))]);

        // 柊秀雪 is the arranger, so his profile does not name the vocalist
        // and he stays an artist.
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
        let out = resolve(tags, bios(&pages));
        let out: Vec<_> = out
            .iter()
            .map(|c| (c.role, c.name.as_str(), c.sort.as_deref()))
            .collect();
        assert_eq!(out, [(Artist, "柊秀雪", None), (Vocalist, "Atori", None)]);
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
        let out = resolve(tags, |name, _| {
            ["ビートまりお", "myu314"]
                .contains(&name)
                .then(|| artist(name))
        });
        let out: Vec<_> = out
            .iter()
            .map(|c| (c.name.as_str(), c.sort.as_deref()))
            .collect();
        assert_eq!(out, [("ビートまりお", Some("Beatmario")), ("myu314", None)]);
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
}
