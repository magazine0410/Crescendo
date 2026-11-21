//! ListenBrainz: which recording a song is, and what MusicBrainz and its
//! listeners know about the artists on it. Sending listens is in [`scrobble`].
//!
//! Everything here starts from one lookup, artist and title (and album) to a
//! MusicBrainz recording, and ListenBrainz answers that only for a signed-in
//! user. Without a connected account there is no match, and so no page link,
//! tags, listener counts or similar artists.
pub mod scrobble;

use crate::db::Db;
use crate::web::{self, Fetch};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

const API_ROOT: &str = "https://api.listenbrainz.org/";
const LABS_ROOT: &str = "https://labs.api.listenbrainz.org/";
/// ListenBrainz allows 30 requests in about 10 seconds.
const EVERY: Duration = Duration::from_millis(350);
/// ListenBrainz refuses a lookup whose names together are longer than this.
const MAX_LOOKUP_CHARS: usize = 250;
/// Songs in one batch lookup (`MAX_LOOKUPS_PER_POST` on the server).
const MAX_LOOKUPS_PER_POST: usize = 50;
/// The data set the ListenBrainz artist page reads its similar artists from.
const SIMILAR_ALGORITHM: &str =
    "session_based_days_7500_session_300_contribution_5_threshold_10_limit_100_filter_True_skip_30";
const MAX_TAGS: usize = 10;
const MAX_SIMILAR: usize = 10;

/// Where requests go. Tests point this at a local server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub api: String,
    pub labs: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: API_ROOT.into(),
            labs: LABS_ROOT.into(),
        }
    }
}

/// A song as its tags name it.
#[derive(Debug, Clone, Copy)]
pub struct Song<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub album: Option<&'a str>,
}

impl Song<'_> {
    fn album(&self) -> &str {
        self.album.map(str::trim).unwrap_or("")
    }

    /// Whether ListenBrainz can be asked about it at all.
    fn askable(&self) -> bool {
        !self.artist.trim().is_empty()
            && !self.title.trim().is_empty()
            && self.artist.chars().count()
                + self.title.chars().count()
                + self.album().chars().count()
                <= MAX_LOOKUP_CHARS
    }

    fn cache_key(&self, kind: &str) -> String {
        web::key(&[kind, self.artist, self.title, self.album()])
    }
}

/// One artist of a matched recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchedArtist {
    pub name: String,
    pub mbid: String,
    /// The artist's Wikidata id ("Q191352"), when MusicBrainz links one.
    pub wikidata: Option<String>,
    /// MusicBrainz tags, most used first.
    pub tags: Vec<String>,
}

/// The recording ListenBrainz matched a song to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Match {
    pub recording_mbid: String,
    pub artists: Vec<MatchedArtist>,
}

impl Match {
    /// The matched artist a tag names, compared as tags are: by case and
    /// spacing only.
    pub fn artist(&self, name: &str) -> Option<&MatchedArtist> {
        let wanted = norm(name);
        self.artists.iter().find(|a| norm(&a.name) == wanted)
    }
}

/// How many people have listened to an artist on ListenBrainz, and how often.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Popularity {
    pub listeners: i64,
    pub plays: i64,
}

fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn authorization(token: &str) -> String {
    format!("Token {token}")
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Two lookups of one song at once would both ask; the second waits for the
/// first and then reads its cached answer. Now Playing asks for the album
/// artist and the credits side by side.
static LOOKUP: Mutex<()> = Mutex::new(());

/// The recording a song matches, with its artists.
pub(crate) fn song_match(db: &Db, endpoints: &Endpoints, token: &str, song: Song) -> Fetch<Match> {
    if !song.askable() {
        return Fetch::Missing;
    }
    let _one_at_a_time = LOOKUP.lock().unwrap_or_else(|e| e.into_inner());
    let answer = web::cached(db, &song.cache_key("lb-match"), || {
        let mut query = vec![
            ("artist_name", song.artist),
            ("recording_name", song.title),
            ("metadata", "true"),
            ("inc", "artist tag"),
        ];
        if !song.album().is_empty() {
            query.push(("release_name", song.album()));
        }
        match web::get_json(
            &format!("{}1/metadata/lookup/", endpoints.api),
            &query,
            &[("Authorization", &authorization(token))],
            EVERY,
        ) {
            Fetch::Found(value) => match parse_match(&value) {
                Some(found) => Fetch::Found(found),
                None => Fetch::Missing,
            },
            Fetch::Missing => Fetch::Missing,
            Fetch::Failed => Fetch::Failed,
        }
    });
    // Song history asks only for the recording: spare it a request.
    let recording = match &answer {
        Fetch::Found(found) => Fetch::Found(found.recording_mbid.clone()),
        Fetch::Missing => Fetch::Missing,
        Fetch::Failed => Fetch::Failed,
    };
    web::store(db, &song.cache_key("lb-recording"), &recording);
    answer
}

fn parse_match(value: &Value) -> Option<Match> {
    let recording_mbid = text(&value["recording_mbid"])?;
    let metadata = &value["metadata"];
    let mut tags: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for tag in metadata["tag"]["artist"].as_array().into_iter().flatten() {
        if let (Some(mbid), Some(name)) = (text(&tag["artist_mbid"]), text(&tag["tag"])) {
            tags.entry(mbid)
                .or_default()
                .push((tag["count"].as_i64().unwrap_or(0), name));
        }
    }
    let mut artists: Vec<MatchedArtist> = metadata["artist"]["artists"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|artist| {
            let mbid = text(&artist["artist_mbid"])?;
            let mut tags = tags.remove(&mbid).unwrap_or_default();
            tags.sort_by_key(|tag| std::cmp::Reverse(tag.0));
            Some(MatchedArtist {
                name: text(&artist["name"])?,
                wikidata: text(&artist["rels"]["wikidata"])
                    .and_then(|url| url.rsplit('/').next().map(str::to_string))
                    .filter(|id| id.starts_with('Q')),
                tags: tags
                    .into_iter()
                    .map(|(_, tag)| tag)
                    .take(MAX_TAGS)
                    .collect(),
                mbid,
            })
        })
        .collect();
    // Without the extra metadata a single artist is still named by the match.
    if artists.is_empty() {
        if let (Some([mbid]), Some(name)) = (
            value["artist_mbids"].as_array().map(Vec::as_slice),
            text(&value["artist_credit_name"]),
        ) {
            artists.extend(text(mbid).map(|mbid| MatchedArtist {
                name,
                mbid,
                wikidata: None,
                tags: Vec::new(),
            }));
        }
    }
    Some(Match {
        recording_mbid,
        artists,
    })
}

/// The recording each song matches, asked for in batches: Song history shows
/// 50 plays a page. A song that could not be asked about comes back `Failed`.
pub(crate) fn recordings(
    db: &Db,
    endpoints: &Endpoints,
    token: &str,
    songs: &[Song],
) -> Vec<Fetch<String>> {
    let mut out: Vec<Fetch<String>> = songs
        .iter()
        .map(|song| {
            if song.askable() {
                web::peek(db, &song.cache_key("lb-recording"))
            } else {
                Fetch::Missing
            }
        })
        .collect();
    let unknown: Vec<usize> = (0..songs.len())
        .filter(|&i| songs[i].askable() && out[i] == Fetch::Failed)
        .collect();
    for batch in unknown.chunks(MAX_LOOKUPS_PER_POST) {
        let recordings: Vec<Value> = batch
            .iter()
            .map(|&i| {
                let song = &songs[i];
                let mut entry = json!({
                    "artist_name": song.artist,
                    "recording_name": song.title,
                });
                if !song.album().is_empty() {
                    entry["release_name"] = song.album().into();
                }
                entry
            })
            .collect();
        let Fetch::Found(answer) = web::post_json(
            &format!("{}1/metadata/lookup/", endpoints.api),
            &[("Authorization", &authorization(token))],
            &json!({ "recordings": recordings }),
            EVERY,
        ) else {
            break;
        };
        let Some(results) = answer.as_array() else {
            break;
        };
        // Each result names the request it answers; one without a recording
        // is a song ListenBrainz does not know.
        let mut found: HashMap<usize, String> = HashMap::new();
        for result in results {
            if let (Some(index), Some(mbid)) = (
                result["index"].as_u64().map(|i| i as usize),
                text(&result["recording_mbid"]),
            ) {
                found.insert(index, mbid);
            }
        }
        for (position, &i) in batch.iter().enumerate() {
            out[i] = match found.remove(&position) {
                Some(mbid) => Fetch::Found(mbid),
                None => Fetch::Missing,
            };
            web::store(db, &songs[i].cache_key("lb-recording"), &out[i]);
        }
    }
    out
}

/// Listener and play counts for artists, by MBID. Unknown artists are absent.
pub(crate) fn popularity(
    db: &Db,
    endpoints: &Endpoints,
    mbids: &[&str],
) -> HashMap<String, Popularity> {
    let key = |mbid: &str| web::key(&["lb-popularity", mbid]);
    let mut out = HashMap::new();
    let mut unknown = Vec::new();
    for &mbid in mbids {
        match web::peek::<Popularity>(db, &key(mbid)) {
            Fetch::Found(found) => {
                out.insert(mbid.to_string(), found);
            }
            Fetch::Missing => {}
            Fetch::Failed => unknown.push(mbid),
        }
    }
    if unknown.is_empty() {
        return out;
    }
    let Fetch::Found(answer) = web::post_json(
        &format!("{}1/popularity/artist", endpoints.api),
        &[],
        &json!({ "artist_mbids": unknown }),
        EVERY,
    ) else {
        return out;
    };
    let mut answered: HashMap<String, Popularity> = HashMap::new();
    for row in answer.as_array().into_iter().flatten() {
        if let (Some(mbid), Some(listeners), Some(plays)) = (
            text(&row["artist_mbid"]),
            row["total_user_count"].as_i64(),
            row["total_listen_count"].as_i64(),
        ) {
            answered.insert(mbid, Popularity { listeners, plays });
        }
    }
    for mbid in unknown {
        let found = match answered.remove(mbid) {
            Some(found) => Fetch::Found(found),
            None => Fetch::Missing,
        };
        web::store(db, &key(mbid), &found);
        if let Fetch::Found(found) = found {
            out.insert(mbid.to_string(), found);
        }
    }
    out
}

/// Artists whose listeners overlap with this one's, closest first.
pub(crate) fn similar_artists(db: &Db, endpoints: &Endpoints, mbid: &str) -> Vec<String> {
    web::cached(db, &web::key(&["lb-similar", mbid]), || {
        let answer = web::get_json(
            &format!("{}similar-artists/json", endpoints.labs),
            &[("artist_mbids", mbid), ("algorithm", SIMILAR_ALGORITHM)],
            &[],
            EVERY,
        );
        let rows = match answer {
            Fetch::Found(rows) => rows,
            Fetch::Missing => return Fetch::Missing,
            Fetch::Failed => return Fetch::Failed,
        };
        let mut scored: Vec<(i64, String)> = rows
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["artist_mbid"].as_str() != Some(mbid))
            .filter_map(|row| Some((row["score"].as_i64().unwrap_or(0), text(&row["name"])?)))
            .collect();
        scored.sort_by_key(|row| std::cmp::Reverse(row.0));
        let names: Vec<String> = scored
            .into_iter()
            .map(|(_, name)| name)
            .take(MAX_SIMILAR)
            .collect();
        if names.is_empty() {
            Fetch::Missing
        } else {
            Fetch::Found(names)
        }
    })
    .found()
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::testing::server;

    fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("lb.db")).unwrap();
        (dir, db)
    }

    fn endpoints(url: &str) -> Endpoints {
        Endpoints {
            api: url.into(),
            labs: url.into(),
        }
    }

    const SONG: Song = Song {
        artist: "Tomoya feat. 鹿乃",
        title: "夜の歌",
        album: Some("アルバム"),
    };

    #[test]
    fn a_match_names_its_artists_with_their_tags_and_wikidata_ids() {
        let (_dir, db) = db();
        let (url, requests) = server(vec![(
            200,
            json!({
                "recording_mbid": "rec-1",
                "artist_mbids": ["a-1", "a-2"],
                "artist_credit_name": "Tomoya feat. 鹿乃",
                "metadata": {
                    "artist": {"artists": [
                        {"name": "Tomoya", "artist_mbid": "a-1", "rels": {}},
                        {"name": "鹿乃", "artist_mbid": "a-2",
                         "rels": {"wikidata": "https://www.wikidata.org/wiki/Q11674871"}}
                    ]},
                    "tag": {"artist": [
                        {"artist_mbid": "a-2", "tag": "j-pop", "count": 2},
                        {"artist_mbid": "a-2", "tag": "utaite", "count": 5}
                    ]}
                }
            }),
        )]);
        let found = song_match(&db, &endpoints(&url), "token-1", SONG)
            .found()
            .unwrap();
        let request = requests.join().unwrap().remove(0);
        assert!(request.starts_with("GET /1/metadata/lookup/?"), "{request}");
        assert!(request
            .to_lowercase()
            .contains("authorization: token token-1"));
        assert!(request.contains("release_name=%E3%82%A2%E3%83%AB%E3%83%90%E3%83%A0"));
        assert_eq!(found.recording_mbid, "rec-1");
        assert!(found.artist("tomoya").unwrap().tags.is_empty());
        let kano = found.artist("鹿乃").unwrap();
        assert_eq!(kano.wikidata.as_deref(), Some("Q11674871"));
        assert_eq!(kano.tags, ["utaite", "j-pop"]);
        // Cached, and Song history's question about the same song is answered.
        assert_eq!(
            song_match(&db, &endpoints("http://127.0.0.1:1/"), "token-1", SONG).found(),
            Some(found)
        );
        assert_eq!(
            recordings(&db, &endpoints("http://127.0.0.1:1/"), "token-1", &[SONG]),
            [Fetch::Found("rec-1".into())]
        );
    }

    #[test]
    fn an_unknown_song_is_remembered_and_a_refused_token_is_not() {
        let (_dir, db) = db();
        let (url, requests) = server(vec![(401, json!({"code": 401})), (200, json!({}))]);
        let at = endpoints(&url);
        assert_eq!(song_match(&db, &at, "bad", SONG), Fetch::Failed);
        assert_eq!(song_match(&db, &at, "good", SONG), Fetch::Missing);
        requests.join().unwrap();
        assert_eq!(
            song_match(&db, &endpoints("http://127.0.0.1:1/"), "good", SONG),
            Fetch::Missing
        );
    }

    #[test]
    fn history_asks_for_uncached_songs_in_one_request() {
        let (_dir, db) = db();
        let other = Song {
            artist: "IOSYS",
            title: "チルノのパーフェクトさんすう教室",
            album: None,
        };
        let too_long = "x".repeat(300);
        let unaskable = Song {
            artist: "A",
            title: &too_long,
            album: None,
        };
        let (url, requests) = server(vec![(
            200,
            json!([
                {"index": 0, "artist_name_arg": "Tomoya feat. 鹿乃"},
                {"index": 1, "recording_mbid": "rec-2"}
            ]),
        )]);
        let at = endpoints(&url);
        assert_eq!(
            recordings(&db, &at, "token-1", &[SONG, unaskable, other]),
            [Fetch::Missing, Fetch::Missing, Fetch::Found("rec-2".into())]
        );
        let request = requests.join().unwrap().remove(0);
        assert!(
            request.starts_with("POST /1/metadata/lookup/ "),
            "{request}"
        );
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["recordings"].as_array().unwrap().len(), 2);
        assert_eq!(body["recordings"][0]["release_name"], "アルバム");
        assert!(body["recordings"][1].get("release_name").is_none());
        // Both answers are cached: no request is made the second time.
        assert_eq!(
            recordings(
                &db,
                &endpoints("http://127.0.0.1:1/"),
                "token-1",
                &[SONG, other]
            ),
            [Fetch::Missing, Fetch::Found("rec-2".into())]
        );
    }

    #[test]
    fn popularity_and_similar_artists_are_read_and_cached() {
        let (_dir, db) = db();
        let (url, requests) = server(vec![
            (
                200,
                json!([
                    {"artist_mbid": "a-1", "total_listen_count": 900, "total_user_count": 40},
                    {"artist_mbid": "a-2", "total_listen_count": null, "total_user_count": null}
                ]),
            ),
            (
                200,
                json!([
                    {"artist_mbid": "b-1", "name": "Second", "score": 10},
                    {"artist_mbid": "b-2", "name": "First", "score": 70},
                    {"artist_mbid": "a-1", "name": "Itself", "score": 99}
                ]),
            ),
        ]);
        let at = endpoints(&url);
        let counts = popularity(&db, &at, &["a-1", "a-2"]);
        assert_eq!(
            counts.get("a-1"),
            Some(&Popularity {
                listeners: 40,
                plays: 900
            })
        );
        assert!(!counts.contains_key("a-2"));
        assert_eq!(similar_artists(&db, &at, "a-1"), ["First", "Second"]);
        requests.join().unwrap();
        let nowhere = endpoints("http://127.0.0.1:1/");
        assert_eq!(popularity(&db, &nowhere, &["a-1", "a-2"]).len(), 1);
        assert_eq!(similar_artists(&db, &nowhere, "a-1"), ["First", "Second"]);
    }
}
