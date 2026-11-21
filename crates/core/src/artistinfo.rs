//! What the Now Playing tab shows about an artist: a profile text from the
//! first site that has a usable one ([`profile`]), and, when ListenBrainz
//! matched the song, the artist's MusicBrainz tags, listener counts and
//! similar artists ([`crate::listenbrainz`]). [`credits`] decides which names
//! in a track's tags get a section of their own.
pub mod credits;
pub mod profile;

use crate::db::Db;
use crate::listenbrainz::{self, Endpoints, Match, Song};
use crate::private_file;
use anyhow::{bail, Result};
use credits::{Credit, CreditTags};
use profile::{Hints, Profile, Sites};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct ArtistInfo {
    pub name: String,
    pub bio: Option<Profile>,
    /// People who have listened to the artist on ListenBrainz, and how often.
    pub listeners: Option<i64>,
    pub playcount: Option<i64>,
    pub tags: Vec<String>,
    pub similar: Vec<String>,
}

/// Where the lookups go and what they may use.
pub struct Lookups<'a> {
    pub db: &'a Db,
    pub sites: &'a Sites,
    pub listenbrainz: &'a Endpoints,
    /// The ListenBrainz user token, when an account is connected.
    pub token: Option<&'a str>,
}

/// The song being played, as its tags name it.
#[derive(Debug, Default, Clone, Copy)]
pub struct Playing<'a> {
    pub artist: Option<&'a str>,
    pub title: Option<&'a str>,
    pub album: Option<&'a str>,
}

impl Lookups<'_> {
    /// The recording ListenBrainz matches the song to, if it can be asked.
    fn song_match(&self, song: Playing) -> Option<Match> {
        listenbrainz::song_match(
            self.db,
            self.listenbrainz,
            self.token?,
            Song {
                artist: song.artist?,
                title: song.title?,
                album: song.album,
            },
        )
        .found()
    }
}

/// Info for the artist a section is headed by: the album artist of a track,
/// or the artist of a song a radio station announced. None when nothing is
/// known, or when online lookups are turned off.
pub fn artist_info(lookups: &Lookups, name: &str, song: Playing) -> Option<ArtistInfo> {
    let name = name.trim();
    if name.is_empty() || !lookups.db.artist_info_online() {
        return None;
    }
    let matched = lookups.song_match(song);
    let artist = matched.as_ref().and_then(|m| m.artist(name));
    let bio = profile::profile(
        lookups.db,
        lookups.sites,
        name,
        Hints {
            album: song.album,
            album_artist: Some(name),
            mbid: artist.map(|a| a.mbid.as_str()),
            wikidata: artist.and_then(|a| a.wikidata.as_deref()),
            trusted: credits::distinctive(name),
        },
    );
    if bio.is_none() && artist.is_none() {
        return None;
    }
    let mut info = ArtistInfo {
        name: name.to_string(),
        bio,
        listeners: None,
        playcount: None,
        tags: Vec::new(),
        similar: Vec::new(),
    };
    if let Some(artist) = artist {
        info.tags = artist.tags.clone();
        let counts = listenbrainz::popularity(lookups.db, lookups.listenbrainz, &[&artist.mbid]);
        if let Some(counts) = counts.get(&artist.mbid) {
            info.listeners = Some(counts.listeners);
            info.playcount = Some(counts.plays);
        }
        info.similar =
            listenbrainz::similar_artists(lookups.db, lookups.listenbrainz, &artist.mbid);
    }
    Some(info)
}

/// Profiles for the names in a track's artist and vocalist tags that the
/// album artist section does not already show. A failed lookup only drops
/// that name.
pub fn credits(lookups: &Lookups, tags: CreditTags) -> Vec<Credit> {
    if !lookups.db.artist_info_online() {
        return Vec::new();
    }
    let matched = lookups.song_match(Playing {
        artist: tags.artist,
        title: tags.title,
        album: tags.album,
    });
    let matched_artist = |name: &str| matched.as_ref().and_then(|m| m.artist(name));
    let mut out = credits::resolve(tags, |name, distinctive| {
        let artist = matched_artist(name);
        let bio = profile::profile(
            lookups.db,
            lookups.sites,
            name,
            Hints {
                album: tags.album,
                album_artist: tags.albumartist,
                mbid: artist.map(|a| a.mbid.as_str()),
                wikidata: artist.and_then(|a| a.wikidata.as_deref()),
                trusted: distinctive,
            },
        )?;
        Some(ArtistInfo {
            name: name.to_string(),
            bio: Some(bio),
            listeners: None,
            playcount: None,
            tags: Vec::new(),
            similar: Vec::new(),
        })
    });
    // Listener counts for the credits ListenBrainz knows, in one request.
    let mbids: Vec<&str> = out
        .iter()
        .filter_map(|credit| matched_artist(&credit.name).map(|a| a.mbid.as_str()))
        .collect();
    if !mbids.is_empty() {
        let counts = listenbrainz::popularity(lookups.db, lookups.listenbrainz, &mbids);
        for credit in &mut out {
            if let Some(counts) = matched_artist(&credit.name).and_then(|a| counts.get(&a.mbid)) {
                credit.info.listeners = Some(counts.listeners);
                credit.info.playcount = Some(counts.plays);
            }
        }
    }
    out
}

// ---- The Discogs account ----

/// A Discogs personal access token and the account it belongs to, kept in a
/// private file. Discogs answers only signed-in requests.
#[derive(Clone, Default, Serialize, Deserialize)]
struct DiscogsAccount {
    token: String,
    username: String,
}

/// The saved Discogs token, if any.
pub fn discogs_token(path: &Path) -> Option<String> {
    private_file::read::<DiscogsAccount>(path)
        .ok()
        .map(|account| account.token)
        .filter(|token| !token.is_empty())
}

/// The Discogs account whose token is saved, if any.
pub fn discogs_username(path: &Path) -> Option<String> {
    private_file::read::<DiscogsAccount>(path)
        .ok()
        .map(|account| account.username)
        .filter(|name| !name.is_empty())
}

/// Saves a Discogs token once Discogs has confirmed it, and returns the
/// account it belongs to. An empty token removes the saved one.
pub fn set_discogs_token(sites: &Sites, path: &Path, token: &str) -> Result<Option<String>> {
    let token = token.trim();
    if token.is_empty() {
        private_file::write(path, &DiscogsAccount::default())?;
        return Ok(None);
    }
    if token.len() > 200 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        bail!("Enter the personal access token from your Discogs developer settings.");
    }
    let mut response = ureq::get(&format!("{}oauth/identity", sites.discogs))
        .header("User-Agent", crate::web::USER_AGENT)
        .header("Authorization", &format!("Discogs token={token}"))
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|_| anyhow::anyhow!("Cannot reach Discogs. Try again later."))?;
    let username = match response.status().as_u16() {
        200..=299 => crate::http::read_json::<serde_json::Value>(response.body_mut())
            .ok()
            .and_then(|identity| identity["username"].as_str().map(str::to_string))
            .filter(|name| !name.is_empty()),
        401 | 403 => bail!("Discogs does not know this token."),
        status => bail!("Discogs returned HTTP {status}. Try again later."),
    };
    let Some(username) = username else {
        bail!("Discogs returned no account for this token.");
    };
    private_file::write(
        path,
        &DiscogsAccount {
            token: token.to_string(),
            username: username.clone(),
        },
    )?;
    Ok(Some(username))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::testing::server;
    use serde_json::json;

    const PROSE: &str = "A doujin music circle that arranges Touhou music as eurobeat. \
                         It has released an album at every Comiket since 2006.";

    fn sites(url: &str) -> Sites {
        Sites {
            touhoudb: format!("{url}touhoudb/api/"),
            vocadb: format!("{url}vocadb/api/"),
            discogs: format!("{url}discogs/"),
            audiodb: format!("{url}audiodb/"),
            wikipedia: format!("{url}wikipedia"),
            wikidata: format!("{url}wikidata"),
            discogs_token: None,
        }
    }

    #[test]
    fn a_matched_artist_gets_tags_counts_and_similar_artists_with_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("info.db")).unwrap();
        let (url, requests) = server(vec![
            // ListenBrainz: the match.
            (
                200,
                json!({"recording_mbid": "rec-1", "metadata": {
                    "artist": {"artists": [{"name": "IOSYS", "artist_mbid": "mb-iosys", "rels": {}}]},
                    "tag": {"artist": [{"artist_mbid": "mb-iosys", "tag": "denpa", "count": 3}]}}}),
            ),
            // TouhouDB: the profile.
            (
                200,
                json!({"items": [{"id": 9, "name": "IOSYS", "description": PROSE}]}),
            ),
            // ListenBrainz: popularity, then similar artists.
            (
                200,
                json!([{"artist_mbid": "mb-iosys", "total_listen_count": 5000, "total_user_count": 300}]),
            ),
            (
                200,
                json!([{"artist_mbid": "mb-2", "name": "COOL&CREATE", "score": 12}]),
            ),
        ]);
        let endpoints = Endpoints {
            api: url.clone(),
            labs: url.clone(),
        };
        let sites = sites(&url);
        let lookups = Lookups {
            db: &db,
            sites: &sites,
            listenbrainz: &endpoints,
            token: Some("token-1"),
        };
        let song = Playing {
            artist: Some("IOSYS"),
            title: Some("チルノのパーフェクトさんすう教室"),
            album: Some("東方氷雪歌集"),
        };
        let info = artist_info(&lookups, "IOSYS", song).unwrap();
        requests.join().unwrap();
        assert_eq!(info.bio.unwrap().text, PROSE);
        assert_eq!(info.tags, ["denpa"]);
        assert_eq!((info.listeners, info.playcount), (Some(300), Some(5000)));
        assert_eq!(info.similar, ["COOL&CREATE"]);

        // With lookups turned off, nothing is asked: the server is gone.
        db.prefs_set("artist_info_online", "false").unwrap();
        assert!(artist_info(&lookups, "Somebody Else", song).is_none());
        let tags = CreditTags {
            artist: Some("Somebody Else"),
            ..Default::default()
        };
        assert!(credits(&lookups, tags).is_empty());
    }

    #[test]
    fn without_an_account_only_the_profile_sites_are_asked() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("info.db")).unwrap();
        let (url, requests) = server(vec![(
            200,
            json!({"items": [{"id": 9, "name": "Nanahira", "description": PROSE}]}),
        )]);
        let endpoints = Endpoints {
            api: "http://127.0.0.1:1/".into(),
            labs: "http://127.0.0.1:1/".into(),
        };
        let sites = sites(&url);
        let lookups = Lookups {
            db: &db,
            sites: &sites,
            listenbrainz: &endpoints,
            token: None,
        };
        let tags = CreditTags {
            albumartist: Some("Various Artists"),
            artist: Some("Nanahira"),
            title: Some("Song"),
            ..Default::default()
        };
        let found = credits(&lookups, tags);
        let requests = requests.join().unwrap();
        // "Various Artists" was asked first, of the same server: one request
        // is all it answers, so only the credit's own lookup can have got it.
        assert_eq!(found.len(), 1, "{requests:?}");
        assert_eq!(found[0].name, "Nanahira");
        assert!(found[0].info.listeners.is_none());
    }

    #[test]
    fn a_discogs_token_is_saved_only_once_discogs_confirms_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discogs.json");
        let (url, requests) = server(vec![
            (401, json!({"message": "Invalid consumer token."})),
            (200, json!({"id": 1, "username": "alice"})),
        ]);
        let sites = sites(&url);
        assert!(set_discogs_token(&sites, &path, "wrong").is_err());
        assert!(discogs_token(&path).is_none());
        assert_eq!(
            set_discogs_token(&sites, &path, " right-token ")
                .unwrap()
                .as_deref(),
            Some("alice")
        );
        let requests = requests.join().unwrap();
        assert!(requests[1].starts_with("GET /discogs/oauth/identity "));
        assert!(requests[1]
            .to_lowercase()
            .contains("authorization: discogs token=right-token\r\n"));
        assert_eq!(discogs_token(&path).as_deref(), Some("right-token"));
        assert_eq!(discogs_username(&path).as_deref(), Some("alice"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // An empty token removes it, without asking Discogs.
        assert!(set_discogs_token(&sites, &path, "").unwrap().is_none());
        assert!(discogs_token(&path).is_none());
    }
}
