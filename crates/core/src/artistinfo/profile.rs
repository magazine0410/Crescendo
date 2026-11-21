//! An artist's profile text for the Now Playing tab, from the first site in a
//! fixed order that has a usable one: TouhouDB, Discogs, TheAudioDB, VocaDB,
//! then English Wikipedia.
//!
//! Two things decide whether a site's text is taken.
//!
//! **Whose it is.** Every site is searched by name, and names are shared: on
//! a sample of this kind of library, a third of Discogs's same-name entries
//! were another artist. So an entry is taken when the site confirms it, by
//! listing the artist on the album being played or by carrying the artist's
//! MusicBrainz id, or, failing that, when it is the site's only entry of that
//! name and the name is long enough to be distinctive ([`Hints::trusted`]).
//!
//! **Whether it says anything.** Many entries hold a member list, a line of
//! links or a note between editors rather than a description. [`usable`]
//! asks for a few sentences, and the next site is tried without them.
use crate::db::Db;
use crate::web::{self, Fetch};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;

/// Sentence text a profile needs, in characters, to count as one.
const MIN_PROSE_CHARS: usize = 50;
/// Same-name entries of one site that are looked at.
const MAX_ENTRIES: usize = 10;

const VOCADB_EVERY: Duration = Duration::from_millis(500);
/// Discogs allows 60 authenticated requests a minute.
const DISCOGS_EVERY: Duration = Duration::from_millis(1100);
/// Discogs's terms forbid showing its data once it is more than six hours
/// older than the site's, or keeping it longer than needed. So its answers
/// are cached for six hours, not the 30 days of the other sites, under keys
/// that start with [`DISCOGS_KEY`] so that old ones can be removed at startup.
pub(crate) const DISCOGS_MAX_AGE_SECS: i64 = 6 * 3600;
pub(crate) const DISCOGS_KEY: &str = "discogs";
/// TheAudioDB's free key allows 30 requests a minute.
const AUDIODB_EVERY: Duration = Duration::from_millis(2100);
const WIKI_EVERY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    #[serde(rename = "TouhouDB")]
    TouhouDb,
    Discogs,
    #[serde(rename = "TheAudioDB")]
    AudioDb,
    #[serde(rename = "VocaDB")]
    VocaDb,
    Wikipedia,
}

impl Source {
    /// The order the sites are asked in.
    pub const ORDER: [Source; 5] = [
        Source::TouhouDb,
        Source::Discogs,
        Source::AudioDb,
        Source::VocaDb,
        Source::Wikipedia,
    ];

    fn key(self) -> &'static str {
        match self {
            Source::TouhouDb => "touhoudb",
            Source::Discogs => "discogs",
            Source::AudioDb => "audiodb",
            Source::VocaDb => "vocadb",
            Source::Wikipedia => "wikipedia",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    pub source: Source,
    pub text: String,
    /// The artist's page on that site.
    pub url: String,
}

/// Where each site is asked, and the Discogs token. Tests point the addresses
/// at a local server.
#[derive(Debug, Clone)]
pub struct Sites {
    pub touhoudb: String,
    pub vocadb: String,
    pub discogs: String,
    pub audiodb: String,
    pub wikipedia: String,
    pub wikidata: String,
    /// Discogs answers only signed-in requests, so it is skipped without one.
    pub discogs_token: Option<String>,
}

impl Default for Sites {
    fn default() -> Self {
        Self {
            touhoudb: "https://touhoudb.com/api/".into(),
            vocadb: "https://vocadb.net/api/".into(),
            discogs: "https://api.discogs.com/".into(),
            // "123" is the key TheAudioDB publishes for free use.
            audiodb: "https://www.theaudiodb.com/api/v1/json/123/".into(),
            wikipedia: "https://en.wikipedia.org/w/api.php".into(),
            wikidata: "https://www.wikidata.org/w/api.php".into(),
            discogs_token: None,
        }
    }
}

/// The only pages a profile's source line opens: an artist's page on one of
/// the sites above, over https, with nothing unusual in the address.
pub fn valid_link(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    let allowed = match url.host_str() {
        Some("touhoudb.com" | "vocadb.net") => url.path().starts_with("/Ar/"),
        Some("www.discogs.com") => url.path().starts_with("/artist/"),
        Some("www.theaudiodb.com") => url.path().starts_with("/artist/"),
        Some("en.wikipedia.org") => url.path().starts_with("/wiki/"),
        _ => false,
    };
    (allowed
        && url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none())
    .then(|| url.into())
}

/// What is known about the artist besides the name.
#[derive(Debug, Default, Clone, Copy)]
pub struct Hints<'a> {
    /// The album being played, and the artist it is filed under.
    pub album: Option<&'a str>,
    pub album_artist: Option<&'a str>,
    /// The artist's MusicBrainz and Wikidata ids, when ListenBrainz matched
    /// the song and names this artist on it.
    pub mbid: Option<&'a str>,
    pub wikidata: Option<&'a str>,
    /// The name is long enough that a site's only entry under it may be taken
    /// without confirmation.
    pub trusted: bool,
}

/// What a site has under a name. `text` and `url` may be empty until the
/// entry has been chosen (Discogs sends them per artist).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    id: String,
    text: String,
    url: String,
    /// The MusicBrainz artist the site says this is.
    mbid: Option<String>,
}

/// The first usable profile for `name`, in [`Source::ORDER`].
pub fn profile(db: &Db, sites: &Sites, name: &str, hints: Hints) -> Option<Profile> {
    let name = name.trim();
    // "Various Artists" is where compilations are filed, not an artist.
    if name.is_empty() || is_various(name) {
        return None;
    }
    Source::ORDER.into_iter().find_map(|source| {
        let entry = match source {
            Source::TouhouDb => vocadb_family(db, &sites.touhoudb, source, name, &hints),
            Source::Discogs => discogs(db, sites, name, &hints),
            Source::AudioDb => audiodb(db, sites, name, &hints),
            Source::VocaDb => vocadb_family(db, &sites.vocadb, source, name, &hints),
            Source::Wikipedia => wikipedia(db, sites, name, &hints),
        }?;
        let text = plain(&entry.text);
        usable(&text).then_some(Profile {
            source,
            text,
            url: entry.url,
        })
    })
}

/// Which of a site's same-name entries is this artist, if that can be told.
/// `on_album` fetches the ids of the artists the site lists on the album.
fn choose(
    mut entries: Vec<Entry>,
    hints: &Hints,
    on_album: impl FnOnce() -> Vec<String>,
) -> Option<Entry> {
    if let Some(mbid) = hints.mbid {
        if let Some(i) = entries.iter().position(|e| e.mbid.as_deref() == Some(mbid)) {
            return Some(entries.swap_remove(i));
        }
        // An entry that names another MusicBrainz artist is someone else.
        entries.retain(|e| e.mbid.is_none());
    }
    match entries.len() {
        0 => return None,
        1 if hints.trusted => return entries.pop(),
        _ => {}
    }
    if hints.album.is_none_or(|album| album.trim().is_empty()) {
        return None;
    }
    let listed = on_album();
    let i = entries.iter().position(|e| listed.contains(&e.id))?;
    Some(entries.swap_remove(i))
}

fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A JSON id, which sites send as a number or as a string.
fn id_of(v: &Value) -> Option<String> {
    v.as_i64().map(|n| n.to_string()).or_else(|| text(v))
}

fn found_unless_empty<T>(items: Vec<T>) -> Fetch<Vec<T>> {
    if items.is_empty() {
        Fetch::Missing
    } else {
        Fetch::Found(items)
    }
}

// ---- TouhouDB and VocaDB: the same software behind the same API ----

fn vocadb_family(db: &Db, root: &str, source: Source, name: &str, hints: &Hints) -> Option<Entry> {
    let wanted = norm(name);
    let entries = web::cached(db, &web::key(&["profile", source.key(), &wanted]), || {
        let answer = web::get_json(
            &format!("{root}artists"),
            &[
                ("query", name),
                ("nameMatchMode", "Exact"),
                ("fields", "Description,AdditionalNames"),
                ("maxResults", "10"),
                ("getTotalCount", "false"),
                ("lang", "Default"),
            ],
            &[],
            VOCADB_EVERY,
        );
        let answer = match answer {
            Fetch::Found(answer) => answer,
            Fetch::Missing => return Fetch::Missing,
            Fetch::Failed => return Fetch::Failed,
        };
        let site = root.trim_end_matches("api/");
        found_unless_empty(
            answer["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| {
                    let names = [&item["name"], &item["defaultName"]]
                        .into_iter()
                        .filter_map(Value::as_str)
                        .chain(item["additionalNames"].as_str().unwrap_or("").split(','));
                    names.into_iter().any(|n| norm(n) == wanted)
                })
                .filter_map(|item| {
                    let id = id_of(&item["id"])?;
                    Some(Entry {
                        text: item["description"].as_str().unwrap_or("").to_string(),
                        url: format!("{site}Ar/{id}"),
                        mbid: None,
                        id,
                    })
                })
                .take(MAX_ENTRIES)
                .collect(),
        )
    })
    .found()?;
    choose(entries, hints, || {
        let album = hints.album.unwrap_or("").trim();
        web::cached(
            db,
            &web::key(&["album-artists", source.key(), &norm(album)]),
            || {
                let answer = web::get_json(
                    &format!("{root}albums"),
                    &[
                        ("query", album),
                        ("nameMatchMode", "Exact"),
                        ("fields", "Artists"),
                        ("maxResults", "5"),
                        ("getTotalCount", "false"),
                        ("lang", "Default"),
                    ],
                    &[],
                    VOCADB_EVERY,
                );
                match answer {
                    Fetch::Found(answer) => found_unless_empty(
                        answer["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .flat_map(|album| album["artists"].as_array().into_iter().flatten())
                            .filter_map(|credit| id_of(&credit["artist"]["id"]))
                            .collect(),
                    ),
                    Fetch::Missing => Fetch::Missing,
                    Fetch::Failed => Fetch::Failed,
                }
            },
        )
        .found()
        .unwrap_or_default()
    })
}

// ---- Discogs ----

/// What compilations are filed under, which no site lists as an artist.
fn is_various(name: &str) -> bool {
    matches!(
        norm(name).as_str(),
        "various artists" | "various" | "va" | "v.a." | "v.a"
    )
}

fn discogs_cached<T: Serialize + serde::de::DeserializeOwned>(
    db: &Db,
    key: &str,
    fetch: impl FnOnce() -> Fetch<T>,
) -> Fetch<T> {
    web::cached_within(db, key, DISCOGS_MAX_AGE_SECS, fetch)
}

/// Discogs tells artists of one name apart with a number: "A-One (6)".
static DISCOGS_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s\(\d+\)$").unwrap());

fn discogs(db: &Db, sites: &Sites, name: &str, hints: &Hints) -> Option<Entry> {
    let token = sites.discogs_token.as_deref().filter(|t| !t.is_empty())?;
    let authorization = format!("Discogs token={token}");
    let root = &sites.discogs;
    let get = |path: &str, query: &[(&str, &str)], accept: &str| {
        web::get_json(
            &format!("{root}{path}"),
            query,
            &[("Authorization", &authorization), ("Accept", accept)],
            DISCOGS_EVERY,
        )
    };
    const JSON: &str = "application/vnd.discogs.v2.discogs+json";
    let wanted = norm(name);
    let ids: Vec<String> =
        discogs_cached(db, &web::key(&[DISCOGS_KEY, "artists", &wanted]), || {
            let answer = get(
                "database/search",
                &[("q", name), ("type", "artist"), ("per_page", "25")],
                JSON,
            );
            match answer {
                Fetch::Found(answer) => found_unless_empty(
                    answer["results"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|result| {
                            result["title"].as_str().is_some_and(|title| {
                                norm(&DISCOGS_NUMBER.replace(title, "")) == wanted
                            })
                        })
                        .filter_map(|result| id_of(&result["id"]))
                        .take(MAX_ENTRIES)
                        .collect(),
                ),
                Fetch::Missing => Fetch::Missing,
                Fetch::Failed => Fetch::Failed,
            }
        })
        .found()?;
    let entries = ids
        .into_iter()
        .map(|id| Entry {
            id,
            text: String::new(),
            url: String::new(),
            mbid: None,
        })
        .collect();
    let chosen = choose(entries, hints, || {
        let album = hints.album.unwrap_or("").trim();
        // The album's own artist finds the release; a compilation is filed
        // under none, so there the name itself has to.
        let filed_under = hints
            .album_artist
            .map(str::trim)
            .filter(|a| !a.is_empty() && !is_various(a))
            .unwrap_or(name);
        discogs_cached(
            db,
            &web::key(&[DISCOGS_KEY, "album", &norm(filed_under), &norm(album)]),
            || {
                let search = get(
                    "database/search",
                    &[
                        ("type", "release"),
                        ("artist", filed_under),
                        ("release_title", album),
                        ("per_page", "1"),
                    ],
                    JSON,
                );
                let release = match search {
                    Fetch::Found(search) => match id_of(&search["results"][0]["id"]) {
                        Some(id) => id,
                        None => return Fetch::Missing,
                    },
                    Fetch::Missing => return Fetch::Missing,
                    Fetch::Failed => return Fetch::Failed,
                };
                match get(&format!("releases/{release}"), &[], JSON) {
                    Fetch::Found(release) => {
                        // Everyone credited: the release, its extra credits,
                        // and each track.
                        let lists = |of: &Value| -> Vec<String> {
                            ["artists", "extraartists"]
                                .into_iter()
                                .flat_map(|key| of[key].as_array().into_iter().flatten())
                                .filter_map(|artist| id_of(&artist["id"]))
                                .collect()
                        };
                        let mut ids = lists(&release);
                        for track in release["tracklist"].as_array().into_iter().flatten() {
                            ids.extend(lists(track));
                        }
                        found_unless_empty(ids)
                    }
                    Fetch::Missing => Fetch::Missing,
                    Fetch::Failed => Fetch::Failed,
                }
            },
        )
        .found()
        .unwrap_or_default()
    })?;
    discogs_cached(db, &web::key(&[DISCOGS_KEY, "artist", &chosen.id]), || {
        // The plain-text form resolves Discogs's own markup ([a=Name],
        // [l123]) into names.
        match get(
            &format!("artists/{}", chosen.id),
            &[],
            "application/vnd.discogs.v2.plaintext+json",
        ) {
            Fetch::Found(artist) => Fetch::Found(Entry {
                id: chosen.id.clone(),
                text: text(&artist["profile_plaintext"])
                    .or_else(|| text(&artist["profile"]))
                    .unwrap_or_default(),
                url: text(&artist["uri"])
                    .unwrap_or_else(|| format!("https://www.discogs.com/artist/{}", chosen.id)),
                mbid: None,
            }),
            Fetch::Missing => Fetch::Missing,
            Fetch::Failed => Fetch::Failed,
        }
    })
    .found()
}

// ---- TheAudioDB ----

fn audiodb(db: &Db, sites: &Sites, name: &str, hints: &Hints) -> Option<Entry> {
    let root = &sites.audiodb;
    let entries_of = |answer: Fetch<Value>, wanted: Option<&str>| -> Fetch<Vec<Entry>> {
        let answer = match answer {
            Fetch::Found(answer) => answer,
            Fetch::Missing => return Fetch::Missing,
            Fetch::Failed => return Fetch::Failed,
        };
        // A search also returns near matches ("Cool" for "COOL&CREATE").
        found_unless_empty(
            answer["artists"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|artist| {
                    wanted
                        .is_none_or(|w| artist["strArtist"].as_str().is_some_and(|n| norm(n) == w))
                })
                .filter_map(|artist| {
                    let id = id_of(&artist["idArtist"])?;
                    Some(Entry {
                        // The English text; the other languages have fields
                        // of their own.
                        text: artist["strBiography"].as_str().unwrap_or("").to_string(),
                        url: format!("https://www.theaudiodb.com/artist/{id}"),
                        mbid: text(&artist["strMusicBrainzID"]),
                        id,
                    })
                })
                .take(MAX_ENTRIES)
                .collect(),
        )
    };
    if let Some(mbid) = hints.mbid {
        let by_id = web::cached(db, &web::key(&["profile", "audiodb", "mbid", mbid]), || {
            entries_of(
                web::get_json(
                    &format!("{root}artist-mb.php"),
                    &[("i", mbid)],
                    &[],
                    AUDIODB_EVERY,
                ),
                None,
            )
        });
        if let Some(entry) = by_id
            .found()
            .and_then(|entries| choose(entries, hints, Vec::new))
        {
            return Some(entry);
        }
    }
    let wanted = norm(name);
    let entries = web::cached(db, &web::key(&["profile", "audiodb", &wanted]), || {
        entries_of(
            web::get_json(
                &format!("{root}search.php"),
                &[("s", name)],
                &[],
                AUDIODB_EVERY,
            ),
            Some(&wanted),
        )
    })
    .found()?;
    // TheAudioDB has no album check worth a request at its rate; an entry is
    // confirmed by its MusicBrainz id or taken on a distinctive name.
    choose(
        entries,
        &Hints {
            album: None,
            ..*hints
        },
        Vec::new,
    )
}

// ---- English Wikipedia ----

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WikiPage {
    title: String,
    extract: String,
    /// Its Wikidata id.
    item: Option<String>,
}

fn wikipedia(db: &Db, sites: &Sites, name: &str, hints: &Hints) -> Option<Entry> {
    let page_of = |title: &str| -> Fetch<WikiPage> {
        web::cached(db, &web::key(&["wikipedia", title]), || {
            let answer = web::get_json(
                &sites.wikipedia,
                &[
                    ("action", "query"),
                    ("titles", title),
                    ("redirects", "1"),
                    ("prop", "extracts|pageprops"),
                    ("exintro", "1"),
                    ("explaintext", "1"),
                    ("format", "json"),
                    ("formatversion", "2"),
                ],
                &[],
                WIKI_EVERY,
            );
            let answer = match answer {
                Fetch::Found(answer) => answer,
                Fetch::Missing => return Fetch::Missing,
                Fetch::Failed => return Fetch::Failed,
            };
            let page = &answer["query"]["pages"][0];
            let props = &page["pageprops"];
            // A page listing the things a name can mean is not about any one.
            if page["missing"].as_bool() == Some(true)
                || page["invalid"].as_bool() == Some(true)
                || props.get("disambiguation").is_some()
            {
                return Fetch::Missing;
            }
            match (text(&page["title"]), text(&page["extract"])) {
                (Some(title), Some(extract)) => Fetch::Found(WikiPage {
                    title,
                    extract,
                    item: text(&props["wikibase_item"]),
                }),
                _ => Fetch::Missing,
            }
        })
    };
    let entry_of = |page: WikiPage, mbid: Option<String>| {
        let mut url = url::Url::parse("https://en.wikipedia.org/wiki/").ok()?;
        url.path_segments_mut()
            .ok()?
            .pop_if_empty()
            .push(&page.title.replace(' ', "_"));
        Some(Entry {
            id: page.title,
            text: page.extract,
            url: url.into(),
            mbid,
        })
    };
    // The page MusicBrainz itself links for this artist, through Wikidata.
    // Its title need not be the name: 上海アリス幻樂団 is "Team Shanghai Alice".
    if let (Some(item), Some(mbid)) = (hints.wikidata, hints.mbid) {
        let title = web::cached(
            db,
            &web::key(&["wikidata-enwiki", item]),
            || match web::get_json(
                &sites.wikidata,
                &[
                    ("action", "wbgetentities"),
                    ("ids", item),
                    ("props", "sitelinks"),
                    ("sitefilter", "enwiki"),
                    ("format", "json"),
                ],
                &[],
                WIKI_EVERY,
            ) {
                Fetch::Found(answer) => {
                    match text(&answer["entities"][item]["sitelinks"]["enwiki"]["title"]) {
                        Some(title) => Fetch::Found(title),
                        None => Fetch::Missing,
                    }
                }
                Fetch::Missing => Fetch::Missing,
                Fetch::Failed => Fetch::Failed,
            },
        )
        .found();
        if let Some(page) = title.and_then(|title| page_of(&title).found()) {
            return entry_of(page, Some(mbid.to_string()));
        }
    }
    // Otherwise the page titled like the name, if it is about a music artist:
    // most titles that match an artist's name are about something else (a
    // town, a butterfly, a word). Wikidata says which MusicBrainz artist a
    // page is about, when it is about one.
    let page = page_of(name).found()?;
    let item = page.item.clone()?;
    let mbids: Vec<String> =
        web::cached(
            db,
            &web::key(&["wikidata-mbid", &item]),
            || match web::get_json(
                &sites.wikidata,
                &[
                    ("action", "wbgetclaims"),
                    ("entity", &item),
                    ("property", "P434"),
                    ("format", "json"),
                ],
                &[],
                WIKI_EVERY,
            ) {
                Fetch::Found(answer) => found_unless_empty(
                    answer["claims"]["P434"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|claim| text(&claim["mainsnak"]["datavalue"]["value"]))
                        .collect(),
                ),
                Fetch::Missing => Fetch::Missing,
                Fetch::Failed => Fetch::Failed,
            },
        )
        .found()?;
    let mbid = hints
        .mbid
        .filter(|known| mbids.iter().any(|m| m == known))
        .map(str::to_string)
        .or_else(|| mbids.into_iter().next());
    choose(
        vec![entry_of(page, mbid)?],
        &Hints {
            album: None,
            ..*hints
        },
        Vec::new,
    )
}

// ---- Is the text a profile? ----

/// `[label](address)` as VocaDB and TouhouDB write links.
static MARKDOWN_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\((?:https?://)[^)\s]*\)").unwrap());
static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*\n]+)\*\*").unwrap());
static ITALIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*([^*\n]+)\*").unwrap());
static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^>\s?").unwrap());
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());
/// A line that starts as a list entry does.
static LIST_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[-*]\s|[•・◆◇■□▼▶●]|\d+[.)]\s)").unwrap());
/// "Members:", "circle: Buta-Otome".
static LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^.。!?！？]{1,30}[:：](?:\s|$)").unwrap());
static ENDS_SENTENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[.!?。！？]["”'’」』]?$"#).unwrap());
/// A sentence addressed to the site's editors or about the entry itself,
/// rather than about the artist: "Possible duplicate of …", "Formerly known
/// as …", "Incorrect tag for らっぷびと".
static NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\W*(?:possibl[ey] (?:a )?duplicate|(?:may|might) be the same|not to be confused with|do(?:es)? not appear to be the same|formerly known as|do not use|don't use|not necessarily|see also|please (?:do not|don't|note)|albums? from \d{4} onward|(?:this is an? )?(?:incorrect|wrong|misspelled) (?:artist )?(?:tag|name|spelling)|misspell|correct (?:tag|name))",
    )
    .unwrap()
});
/// A text shared by several artists of one name: "There are multiple artists
/// with this name", a numbered list of them, or a pointer to another one
/// ("For the Japanese artist see たま"). It says nothing about which of them
/// this is. TheAudioDB holds many, copied from Last.fm.
static SHARED_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*there (?:are|is|were)\b[^.:\n]{0,80}?\b(?:artists?|bands?|acts?|groups?|musicians?|projects?)\b|^\s*1(?:\)|\.\s)|^\s*for the [^.\n]{0,60}?\bsee\b|^\s*the (?:artist )?name\b[^.\n]{0,60}?\bcan refer to\b",
    )
    .unwrap()
});

/// A site's text as plain text: links reduced to their labels and Markdown
/// emphasis removed.
fn plain(text: &str) -> String {
    let text = MARKDOWN_LINK.replace_all(text, "$1");
    let text = BOLD.replace_all(&text, "$1");
    let text = ITALIC.replace_all(&text, "$1");
    QUOTE.replace_all(&text, "").trim().to_string()
}

/// Splits a line after each sentence end that is followed by a space.
fn sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = line.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if matches!(c, '.' | '!' | '?' | '。' | '！' | '？') {
            if let Some(&(next, following)) = chars.peek() {
                if following.is_whitespace() {
                    out.push(line[start..next].trim());
                    start = next;
                }
            }
        }
    }
    out.push(line[start..].trim());
    out
}

/// How much of a text is sentences about the artist, in characters. Left out
/// are web addresses, list entries, lines that do not end a sentence and are
/// short or a "label: value" pair (names, roles), and notes between editors.
fn prose_chars(text: &str) -> usize {
    let text = ADDRESS.replace_all(text, "");
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty() && !LIST_ITEM.is_match(line))
        .filter(|line| {
            ENDS_SENTENCE.is_match(line) || (!LABEL.is_match(line) && line.chars().count() >= 60)
        })
        .map(|line| {
            sentences(&line)
                .into_iter()
                .filter(|sentence| !NOTE.is_match(sentence))
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .count()
        })
        .sum()
}

/// Whether a text reads as a description of one artist.
pub(crate) fn usable(text: &str) -> bool {
    !SHARED_NAME.is_match(text) && prose_chars(text) >= MIN_PROSE_CHARS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::testing::server;
    use serde_json::json;

    fn new_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("profiles.db")).unwrap();
        (dir, db)
    }

    /// Every site at one local server, which answers in the order asked.
    fn sites(url: &str) -> Sites {
        Sites {
            touhoudb: format!("{url}touhoudb/api/"),
            vocadb: format!("{url}vocadb/api/"),
            discogs: format!("{url}discogs/"),
            audiodb: format!("{url}audiodb/"),
            wikipedia: format!("{url}wikipedia"),
            wikidata: format!("{url}wikidata"),
            discogs_token: Some("discogs-token".into()),
        }
    }

    const PROSE: &str = "A doujin music circle that arranges Touhou music as eurobeat. \
                         It has released an album at every Comiket since 2006.";

    fn nothing() -> (u16, Value) {
        (
            200,
            json!({"items": [], "results": [], "artists": null, "query": {"pages": [{"missing": true}]}}),
        )
    }

    // Texts below are from entries found for this library's artists.

    #[test]
    fn descriptions_are_usable() {
        for text in [
            "A defunct doujin music circle that was active between the years 2006 and 2011. In 2012, their circle name changed.",
            "Yura Hatsuki. Japanese female singer. Born in Kobe.",
            "Tamaonsen. Touhou-focus doujin circle. Main genre: Hip-Hop / Trip-Hop",
            "サークル外柿山〈とがきさん〉では、東方耳コピmidi、東方オーケストラアレンジ、東方純邦楽アレンジを取り扱っています。",
            "A vocalist who mainly appears in White Elephant works.",
            "Primarily an Ambient/Pop music group, they often combine vocals with Electronic music.",
        ] {
            assert!(usable(&plain(text)), "{text}");
        }
    }

    #[test]
    fn lists_labels_links_and_notes_are_not() {
        for text in [
            "Former members:\n* [沙](https://touhoudb.com/Ar/10855)",
            "Members:\n嵯峨飛鳥 (Saga Asuka): Illustration, Leader\n兎明: Vocals\nsroa: Vocals\nIceon: Arrangement",
            "Current members:\narranger, lead guitar:  Master-Dragon / IRON-CHINO \nfemale vocals, arranger, lyrics: 勇舞",
            "J-Metal/Rock band\n朱美/Akemi (Vo.)\n窪田道元/Dougen Kubota (Vo.)\n仁耶/Jinya(ex-炭酸/Tansan) (Git.)",
            "◆Vocal / Lyric / Illust\nめいど\n◆Project / Arrange / Recording / Mixing / Mastering / Guitar / Bass\npizuya",
            "circle: Buta-Otome\nmember: ランコ (vocal), Comp (Bass,guitar,programming), Paprika (piano)",
            "Formerly known as [いえろ～ぜぶら](https://touhoudb.com/Ar/3948) (Yellow Zebra).",
            "Possible duplicate of a member of [そらいろパラソル](https://touhoudb.com/Ar/15342), as they uploaded one song.",
            "May be the same organization as [2nd Flush](https://touhoudb.com/Ar/2056).",
            "Not to be confused with [越田Rute隆人 (Rute)](https://touhoudb.com/Ar/241). Member of Sync with Lix.",
            "DO NOT USE THIS ARTIST UNLESS THE CIRCLE APPEARS ON A DOUJIN COMPILATION LIKE MELONBOOKS OR TORANOANA.",
            "Japanese doujin circle formed in 2012. Official web page: http://akatsuki-records.com/",
            "Incorrect tag for らっぷびと",
            "WRONG TAG for 溝口肇! The right page has his albums and his biography.",
            "http://www.last.fm/music/%E9%BC%93%E5%A4%AA%E8%9D%8B",
            "",
        ] {
            assert!(!usable(&plain(text)), "{text}");
        }
    }

    #[test]
    fn a_text_shared_by_artists_of_one_name_is_not_a_profile() {
        for text in [
            "There are at least fifteen (15) bands or artists named 'Sin'. Some of them are described below, in no particular order of importance.",
            "There is more than one artist with this name:  1) Noah Carter is a singer from Denmark who released two albums.",
            "There are multiple artists called ARU. One of them is a Japanese doujin arranger active since 2010.",
            "There are several artists with the name Cool:\n\n1)  -  Cool (쿨) is a South Korean ballad/k-pop group that first appeared in 1994.",
            "For the Japanese artist see たま\n\nBand that fuses West African music with funk, formed in London in 1998.",
            "1) Japanese arranger, DJ, composer. Circle: DiGiTAL WiNG  2) Katsu is a Finnish rapper from Helsinki.",
            "The Artist name Audy can refer to two artists; 1) Paula Allodya Item or well-known as Audy is a pop singer from Jakarta, Indonesia.",
        ] {
            assert!(!usable(text), "{text}");
        }
        for text in [
            "ほたる (Hotaru) was the prime vocalist of the circle Halozy until its hiatus in 2017.",
            "There were no releases before 2010, when the circle formed. It has put out one album a year since.",
        ] {
            assert!(usable(text), "{text}");
        }
    }

    #[test]
    fn source_links_are_restricted_to_the_profile_sites() {
        for url in [
            "https://touhoudb.com/Ar/92",
            "https://vocadb.net/Ar/462",
            "https://www.discogs.com/artist/4368544-A-One-6",
            "https://www.theaudiodb.com/artist/183529",
            "https://en.wikipedia.org/wiki/Team_Shanghai_Alice",
        ] {
            assert_eq!(valid_link(url).as_deref(), Some(url));
        }
        for url in [
            "http://touhoudb.com/Ar/92",
            "https://touhoudb.com/User/Login",
            "https://touhoudb.com.evil.test/Ar/92",
            "https://a@vocadb.net/Ar/1",
            "https://www.discogs.com:444/artist/1",
            "https://ja.wikipedia.org/wiki/X",
            "file:///Ar/1",
        ] {
            assert!(valid_link(url).is_none(), "{url}");
        }
    }

    #[test]
    fn links_and_emphasis_are_shown_as_plain_text() {
        assert_eq!(
            plain("> **Kadokawa Corporation**, formerly *Kadokawa Dwango*, see [CYTOKINE](https://vocadb.net/Ar/462)'s page."),
            "Kadokawa Corporation, formerly Kadokawa Dwango, see CYTOKINE's page."
        );
        // A lone asterisk in a name is not emphasis.
        assert_eq!(
            plain("あやぽんず* and Daydream_MusicBoxVer"),
            "あやぽんず* and Daydream_MusicBoxVer"
        );
    }

    fn entry(id: &str, mbid: Option<&str>) -> Entry {
        Entry {
            id: id.into(),
            text: String::new(),
            url: String::new(),
            mbid: mbid.map(str::to_string),
        }
    }

    #[test]
    fn an_entry_is_taken_when_confirmed_or_alone_under_a_distinctive_name() {
        let album = Hints {
            album: Some("KARMA SCREEN"),
            ..Default::default()
        };
        let trusted = Hints {
            trusted: true,
            ..album
        };
        let no_album = || -> Vec<String> { panic!("the album was asked for") };
        // Alone and distinctive: taken without asking about the album.
        assert_eq!(
            choose(vec![entry("1", None)], &trusted, no_album)
                .unwrap()
                .id,
            "1"
        );
        // A short name, or several entries, needs the album to list one.
        assert!(choose(vec![entry("1", None)], &album, Vec::new).is_none());
        assert_eq!(
            choose(vec![entry("1", None)], &album, || vec!["1".into()])
                .unwrap()
                .id,
            "1"
        );
        let several = || vec![entry("1", None), entry("2", None), entry("3", None)];
        assert!(choose(several(), &trusted, Vec::new).is_none());
        assert_eq!(
            choose(several(), &trusted, || vec!["9".into(), "2".into()])
                .unwrap()
                .id,
            "2"
        );
        // Without an album there is nothing to confirm with.
        assert!(choose(
            several(),
            &Hints {
                trusted: true,
                ..Default::default()
            },
            no_album
        )
        .is_none());
        // The MusicBrainz id settles it, either way.
        let known = Hints {
            mbid: Some("mb-2"),
            ..trusted
        };
        let with_ids = vec![entry("1", Some("mb-1")), entry("2", Some("mb-2"))];
        assert_eq!(choose(with_ids, &known, no_album).unwrap().id, "2");
        assert!(choose(vec![entry("1", Some("mb-1"))], &known, Vec::new).is_none());
    }

    #[test]
    fn the_first_site_with_a_usable_profile_wins() {
        let (_dir, db) = new_db();
        let hints = Hints {
            trusted: true,
            ..Default::default()
        };
        // TouhouDB knows the circle but holds only a member list; Discogs
        // has a description.
        let (url, requests) = server(vec![
            (
                200,
                json!({"items": [{"id": 92, "name": "A-One", "additionalNames": "A1",
                                   "description": "Members:\n* Yassie\n* ELEMENTAS"}]}),
            ),
            (
                200,
                json!({"results": [{"id": 4368544, "title": "A-One (6)"},
                                    {"id": 3471743, "title": "DJ A-One"}]}),
            ),
            (
                200,
                json!({"id": 4368544, "profile": "[a=Yassie] and …", "profile_plaintext": PROSE,
                       "uri": "https://www.discogs.com/artist/4368544-A-One-6"}),
            ),
        ]);
        let found = profile(&db, &sites(&url), "A-One", hints).unwrap();
        let requests = requests.join().unwrap();
        assert_eq!(found.source, Source::Discogs);
        assert_eq!(found.text, PROSE);
        assert_eq!(found.url, "https://www.discogs.com/artist/4368544-A-One-6");
        assert!(
            requests[0].starts_with("GET /touhoudb/api/artists?query=A-One&nameMatchMode=Exact")
        );
        assert!(requests[1].starts_with("GET /discogs/database/search?q=A-One&type=artist"));
        assert!(requests[1]
            .to_lowercase()
            .contains("authorization: discogs token=discogs-token"));
        assert!(requests[2].starts_with("GET /discogs/artists/4368544 "));
        assert!(requests[2].contains("application/vnd.discogs.v2.plaintext+json"));
        // Every answer is cached: asked again, no site is.
        let again = profile(&db, &sites("http://127.0.0.1:1/"), "a-one", hints).unwrap();
        assert_eq!(again, found);
        assert_eq!(serde_json::to_value(&found).unwrap()["source"], "Discogs");
    }

    #[test]
    fn a_shared_name_is_settled_by_the_album() {
        let (_dir, db) = new_db();
        let hints = Hints {
            album: Some("KARMA SCREEN"),
            album_artist: Some("A-One"),
            trusted: true,
            ..Default::default()
        };
        let (url, requests) = server(vec![
            nothing(), // TouhouDB has no such artist
            (
                200,
                json!({"results": [{"id": 87275, "title": "A-One"},
                                    {"id": 4368544, "title": "A-One (6)"}]}),
            ),
            (
                200,
                json!({"results": [{"id": 12432464, "title": "A1* - Karma Screen"}]}),
            ),
            (
                200,
                json!({"artists": [{"id": 4368544, "name": "A-One (6)"}],
                       "extraartists": [{"id": 721815, "name": "Zun"}],
                       "tracklist": [{"artists": [{"id": 6664958, "name": "Neiziro"}]}]}),
            ),
            (200, json!({"id": 4368544, "profile_plaintext": PROSE})),
        ]);
        let found = profile(&db, &sites(&url), "A-One", hints).unwrap();
        let requests = requests.join().unwrap();
        assert_eq!(found.url, "https://www.discogs.com/artist/4368544");
        assert!(requests[2].starts_with(
            "GET /discogs/database/search?type=release&artist=A-One&release_title=KARMA%20SCREEN"
        ));
        assert!(requests[3].starts_with("GET /discogs/releases/12432464 "));
        // Another artist on the same album reuses what the album answered.
        let (url, requests) = server(vec![
            nothing(),
            (
                200,
                json!({"results": [{"id": 6664958, "title": "Neiziro"}, {"id": 1, "title": "Neiziro (2)"}]}),
            ),
            (200, json!({"id": 6664958, "profile_plaintext": PROSE})),
        ]);
        let other = profile(&db, &sites(&url), "Neiziro", hints).unwrap();
        requests.join().unwrap();
        assert_eq!(other.url, "https://www.discogs.com/artist/6664958");
    }

    #[test]
    fn a_short_name_needs_the_site_to_list_it_on_the_album() {
        let (_dir, db) = new_db();
        let hints = Hints {
            album: Some("さかさのさかさ"),
            ..Default::default()
        };
        let artist = json!({"items": [{"id": 300, "name": "あき", "description": PROSE}]});
        let (url, requests) = server(vec![
            (200, artist.clone()),
            (
                200,
                json!({"items": [{"name": "さかさのさかさ", "artists": [{"artist": {"id": 300}}, {"name": "guest"}]}]}),
            ),
        ]);
        let mut at = sites(&url);
        at.discogs_token = None;
        let found = profile(&db, &at, "あき", hints).unwrap();
        let requests = requests.join().unwrap();
        assert_eq!(found.source, Source::TouhouDb);
        assert_eq!(found.url, format!("{url}touhoudb/Ar/300"));
        assert!(requests[1].starts_with("GET /touhoudb/api/albums?query="));
    }

    #[test]
    fn without_a_token_discogs_is_not_asked_and_a_failed_site_is_passed_over() {
        let (_dir, db) = new_db();
        let hints = Hints {
            trusted: true,
            ..Default::default()
        };
        // TouhouDB fails, Discogs is skipped, TheAudioDB answers.
        let (failing, failed) = server(vec![(503, json!({}))]);
        let (url, requests) = server(vec![(
            200,
            json!({"artists": [
                {"idArtist": "1", "strArtist": "Cool", "strBiography": PROSE},
                {"idArtist": "183529", "strArtist": "COOL&CREATE", "strBiography": PROSE,
                 "strMusicBrainzID": "bdcab79b"}
            ]}),
        )]);
        let mut at = sites(&url);
        at.touhoudb = format!("{failing}touhoudb/api/");
        at.discogs_token = None;
        let found = profile(&db, &at, "COOL&CREATE", hints).unwrap();
        failed.join().unwrap();
        let requests = requests.join().unwrap();
        assert_eq!(found.source, Source::AudioDb);
        assert_eq!(found.url, "https://www.theaudiodb.com/artist/183529");
        assert!(requests[0].starts_with("GET /audiodb/search.php?s=COOL%26CREATE "));
        // The failure was not taken for an answer: TouhouDB is asked again
        // once it is back (here it is still being left alone, so nothing is).
        assert_eq!(
            web::peek::<Vec<Entry>>(&db, &web::key(&["profile", "touhoudb", "cool&create"])),
            Fetch::Failed
        );
    }

    #[test]
    fn a_musicbrainz_id_finds_and_rules_out_entries() {
        let (_dir, db) = new_db();
        let hints = Hints {
            mbid: Some("mb-right"),
            wikidata: Some("Q1377647"),
            trusted: true,
            ..Default::default()
        };
        let (url, requests) = server(vec![
            nothing(), // TouhouDB
            // TheAudioDB by MusicBrainz id: no such artist, then by name: an
            // entry for another MusicBrainz artist.
            (200, json!({"artists": null})),
            (
                200,
                json!({"artists": [{"idArtist": "5", "strArtist": "上海アリス幻樂団",
                                       "strBiography": PROSE, "strMusicBrainzID": "mb-other"}]}),
            ),
            nothing(), // VocaDB
            // Wikipedia through the Wikidata item MusicBrainz links.
            (
                200,
                json!({"entities": {"Q1377647": {"sitelinks": {"enwiki": {"title": "Team Shanghai Alice"}}}}}),
            ),
            (
                200,
                json!({"query": {"pages": [{"title": "Team Shanghai Alice", "extract": PROSE,
                                              "pageprops": {"wikibase_item": "Q1377647"}}]}}),
            ),
        ]);
        let mut at = sites(&url);
        at.discogs_token = None;
        let found = profile(&db, &at, "上海アリス幻樂団", hints).unwrap();
        let requests = requests.join().unwrap();
        assert_eq!(found.source, Source::Wikipedia);
        assert_eq!(
            found.url,
            "https://en.wikipedia.org/wiki/Team_Shanghai_Alice"
        );
        assert!(requests[1].starts_with("GET /audiodb/artist-mb.php?i=mb-right "));
        assert!(requests[4]
            .starts_with("GET /wikidata?action=wbgetentities&ids=Q1377647&props=sitelinks"));
    }

    #[test]
    fn a_wikipedia_title_counts_only_for_a_music_artist() {
        let hints = Hints {
            trusted: true,
            ..Default::default()
        };
        let page = |title: &str| {
            json!({"query": {"pages": [{"title": title, "extract": PROSE,
                                         "pageprops": {"wikibase_item": "Q1"}}]}})
        };
        // "Veto" is a page about a legal power: no MusicBrainz artist.
        let (_dir, db) = new_db();
        let (url, requests) = server(vec![
            nothing(),
            nothing(),
            nothing(),
            (200, page("Veto")),
            (200, json!({"claims": {}})),
        ]);
        let mut at = sites(&url);
        at.discogs_token = None;
        assert!(profile(&db, &at, "Veto Power", hints).is_none());
        requests.join().unwrap();
        // A band's page names its MusicBrainz artist.
        let (_dir, db) = new_db();
        let (url, requests) = server(vec![
            nothing(),
            nothing(),
            nothing(),
            (200, page("Blind Guardian")),
            (
                200,
                json!({"claims": {"P434": [{"mainsnak": {"datavalue": {"value": "mb-bg"}}}]}}),
            ),
        ]);
        let mut at = sites(&url);
        at.discogs_token = None;
        let found = profile(&db, &at, "Blind Guardian", hints).unwrap();
        let requests = requests.join().unwrap();
        assert_eq!(found.source, Source::Wikipedia);
        assert_eq!(found.url, "https://en.wikipedia.org/wiki/Blind_Guardian");
        assert!(requests[4].starts_with("GET /wikidata?action=wbgetclaims&entity=Q1&property=P434"));
        // A page listing what a name can mean is not one.
        let (_dir, db) = new_db();
        let (url, requests) = server(vec![
            nothing(),
            nothing(),
            nothing(),
            (
                200,
                json!({"query": {"pages": [{"title": "Tsukasa", "extract": PROSE,
                         "pageprops": {"wikibase_item": "Q2", "disambiguation": ""}}]}}),
            ),
        ]);
        let mut at = sites(&url);
        at.discogs_token = None;
        assert!(profile(&db, &at, "Tsukasa", hints).is_none());
        requests.join().unwrap();
    }
}
