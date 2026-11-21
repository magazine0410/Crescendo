//! Where a played song can be opened on a listening service, for Song history.
//!
//! Plays from before ListenBrainz replaced Last.fm keep their Last.fm page.
//! Last.fm's API is no longer called, so those pages come from what the
//! earlier versions left in the database: the track addresses they had cached,
//! and the plays they had scrobbled, whose address follows from the tags.
//! Later plays link to the ListenBrainz page of the recording they match.

use crate::db::Db;
use crate::listenbrainz::{self, Endpoints, Song};
use crate::model::Track;
use crate::web::Fetch;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

/// The `meta` key holding when ListenBrainz took over, in epoch seconds.
pub const SINCE_KEY: &str = "listenbrainz_since";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Lastfm,
    Listenbrainz,
}

/// The only pages Song history opens: a Last.fm music page or a ListenBrainz
/// track page, over https, with nothing unusual in the address.
pub fn valid_link(raw: &str) -> Option<String> {
    let mut url = url::Url::parse(raw).ok()?;
    let allowed = match url.host_str() {
        Some("www.last.fm" | "last.fm") => url.path().starts_with("/music/"),
        Some("listenbrainz.org") => url.path().starts_with("/track/"),
        _ => false,
    };
    if !allowed
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    url.set_scheme("https").ok()?;
    Some(url.into())
}

/// The Last.fm page of a track, from its tags. Last.fm finds the page whatever
/// the capitalization, writes a space as `+`, and so needs a real plus sign
/// encoded twice.
pub fn lastfm_track_url(artist: &str, title: &str) -> String {
    let part = |s: &str| {
        url::form_urlencoded::byte_serialize(s.as_bytes())
            .collect::<String>()
            .replace("%2B", "%252B")
    };
    format!(
        "https://www.last.fm/music/{}/_/{}",
        part(artist),
        part(title)
    )
}

/// The ListenBrainz page of a recording.
pub fn listenbrainz_track_url(recording_mbid: &str) -> String {
    format!("https://listenbrainz.org/track/{recording_mbid}/")
}

/// ListenBrainz pages for the songs it matches, by track id.
#[derive(Debug, Default, Serialize)]
pub struct ListenbrainzLinks {
    pub urls: HashMap<i64, String>,
    /// Some songs could not be asked about (no connection, a rejected
    /// token), so a song without a page here is not known to have none.
    pub failed: bool,
}

/// Looks up the ListenBrainz page of each track, for the plays Song history
/// shows from after the switch.
pub fn listenbrainz_links(
    db: &Db,
    endpoints: &Endpoints,
    token: &str,
    tracks: &[Track],
) -> ListenbrainzLinks {
    let tagged: Vec<(&Track, Song)> = tracks
        .iter()
        .filter_map(|track| {
            Some((
                track,
                Song {
                    artist: track.artist.as_deref()?,
                    title: track.title.as_deref()?,
                    album: track.album.as_deref(),
                },
            ))
        })
        .collect();
    let songs: Vec<Song> = tagged.iter().map(|(_, song)| *song).collect();
    let mut links = ListenbrainzLinks::default();
    for ((track, _), answer) in tagged
        .iter()
        .zip(listenbrainz::recordings(db, endpoints, token, &songs))
    {
        match answer {
            Fetch::Found(mbid) => {
                links.urls.insert(track.id, listenbrainz_track_url(&mbid));
            }
            Fetch::Missing => {}
            Fetch::Failed => links.failed = true,
        }
    }
    links
}

/// Copies every Last.fm track page the earlier versions knew into
/// `lastfm_links`, which nothing expires. Runs once, in the migration that
/// creates the table: `lastfm_cache` dropped its rows after 30 days.
pub(crate) fn keep_lastfm_links(tx: &rusqlite::Transaction) -> Result<()> {
    let has_table = |name: &str| -> rusqlite::Result<bool> {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )
    };
    let mut insert =
        tx.prepare("INSERT OR IGNORE INTO lastfm_links(artist, title, url) VALUES (?1, ?2, ?3)")?;
    // The addresses Last.fm itself returned come first, so they win.
    if has_table("lastfm_cache")? {
        let mut cached =
            tx.prepare("SELECT key, json FROM lastfm_cache WHERE key LIKE 'track:%'")?;
        let rows =
            cached.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (key, json) = row?;
            let tags = serde_json::from_str::<(String, String)>(&key["track:".len()..]);
            let url = serde_json::from_str::<Option<String>>(&json);
            if let (Ok((artist, title)), Ok(Some(url))) = (tags, url) {
                if let Some(url) = valid_link(&url) {
                    insert.execute([&artist, &title, &url])?;
                }
            }
        }
    }
    // A scrobble was only ever sent for a track Last.fm had a page for.
    if has_table("lastfm_outbox")? {
        let mut sent =
            tx.prepare("SELECT track_json FROM lastfm_outbox WHERE state = 'submitted'")?;
        let rows = sent.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            let Ok(track) = serde_json::from_str::<Track>(&row?) else {
                continue;
            };
            if let (Some(artist), Some(title)) = (track.artist, track.title) {
                insert.execute([&artist, &title, &lastfm_track_url(&artist, &title)])?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_restricted_to_the_two_services() {
        for url in [
            "https://evil.test/music/A",
            "https://www.last.fm.evil.test/music/A",
            "https://a@www.last.fm/music/A",
            "file:///music/A",
            "https://www.last.fm/login",
            "https://www.last.fm:444/music/A",
            "https://listenbrainz.org/settings/",
            "https://api.listenbrainz.org/track/x/",
        ] {
            assert!(valid_link(url).is_none(), "{url}");
        }
        assert_eq!(
            valid_link("http://last.fm/music/A/_/B").as_deref(),
            Some("https://last.fm/music/A/_/B")
        );
        let page = listenbrainz_track_url("e97f805a-ab48-4c52-855e-07049142113d");
        assert_eq!(valid_link(&page), Some(page));
    }

    #[test]
    fn a_rebuilt_address_is_one_the_link_check_accepts() {
        let url = lastfm_track_url("COOL&CREATE", "Scream out! (TOS Remix) 夜/朝");
        assert_eq!(
            url,
            "https://www.last.fm/music/COOL%26CREATE/_/\
             Scream+out%21+%28TOS+Remix%29+%E5%A4%9C%2F%E6%9C%9D"
        );
        assert_eq!(valid_link(&url), Some(url));
        // A plus sign in a name must not read as a space.
        assert!(lastfm_track_url("t+pazolite", "x").contains("/t%252Bpazolite/"));
    }
}
