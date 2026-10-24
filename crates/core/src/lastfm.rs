//! Minimal Last.fm API client (artist.getInfo / album.getInfo) with a
//! 30-day response cache in SQLite. Callers fall back to local tags when
//! this returns None (no key, offline, or no result).

use crate::db::Db;
pub mod credits;
pub mod scrobble;
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

const API_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";
pub(crate) const CACHE_TTL_SECS: i64 = 30 * 24 * 3600;

#[derive(Debug, Clone, Serialize)]
pub struct ArtistInfo {
    pub name: String,
    pub bio_summary: Option<String>,
    pub bio_full: Option<String>,
    pub listeners: Option<i64>,
    pub playcount: Option<i64>,
    pub tags: Vec<String>,
    pub similar: Vec<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AlbumInfo {
    pub name: String,
    pub artist: String,
    pub summary: Option<String>,
    pub full: Option<String>,
    pub listeners: Option<i64>,
    pub playcount: Option<i64>,
    pub tags: Vec<String>,
    pub url: Option<String>,
}

pub fn artist_info(db: &Db, api_key: &str, artist: &str) -> Result<Option<ArtistInfo>> {
    if api_key.is_empty() || artist.is_empty() {
        return Ok(None);
    }
    let cache_key = format!("artist\u{1f}{}", artist.to_lowercase());
    let json = match cached_or_fetch(
        db,
        &cache_key,
        api_key,
        &[("method", "artist.getinfo"), ("artist", artist)],
    )? {
        Some(j) => j,
        None => return Ok(None),
    };
    let a = &json["artist"];
    if a.is_null() {
        return Ok(None);
    }
    Ok(Some(ArtistInfo {
        name: str_of(&a["name"]).unwrap_or_else(|| artist.to_string()),
        bio_summary: str_of(&a["bio"]["summary"]).map(strip_lastfm_link),
        bio_full: str_of(&a["bio"]["content"]).map(strip_lastfm_link),
        listeners: int_of(&a["stats"]["listeners"]),
        playcount: int_of(&a["stats"]["playcount"]),
        tags: names_of(&a["tags"]["tag"]),
        similar: names_of(&a["similar"]["artist"]),
        url: str_of(&a["url"]),
    }))
}

pub fn album_info(db: &Db, api_key: &str, artist: &str, album: &str) -> Result<Option<AlbumInfo>> {
    let Some(a) = album_json(db, api_key, artist, album)? else {
        return Ok(None);
    };
    Ok(Some(AlbumInfo {
        name: str_of(&a["name"]).unwrap_or_else(|| album.to_string()),
        artist: str_of(&a["artist"]).unwrap_or_else(|| artist.to_string()),
        summary: str_of(&a["wiki"]["summary"]).map(strip_lastfm_link),
        full: str_of(&a["wiki"]["content"]).map(strip_lastfm_link),
        listeners: int_of(&a["listeners"]),
        playcount: int_of(&a["playcount"]),
        tags: names_of(&a["tags"]["tag"]),
        url: str_of(&a["url"]),
    }))
}

/// The `album` object of an album.getInfo response, or None when Last.fm has
/// no such album under that artist.
fn album_json(db: &Db, api_key: &str, artist: &str, album: &str) -> Result<Option<Value>> {
    if api_key.is_empty() || artist.is_empty() || album.is_empty() {
        return Ok(None);
    }
    let cache_key = format!(
        "album\u{1f}{}\u{1f}{}",
        artist.to_lowercase(),
        album.to_lowercase()
    );
    let json = cached_or_fetch(
        db,
        &cache_key,
        api_key,
        &[
            ("method", "album.getinfo"),
            ("artist", artist),
            ("album", album),
        ],
    )?;
    Ok(json
        .and_then(|mut j| j.get_mut("album").map(Value::take))
        .filter(|a| !a.is_null()))
}

/// Quick key check for the settings dialog "Test key" button.
pub fn test_key(api_key: &str) -> Result<bool> {
    let resp = ureq::get(API_ROOT)
        .query("method", "artist.getinfo")
        .query("artist", "cher")
        .query("api_key", api_key)
        .query("format", "json")
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .call();
    // Only Last.fm's verdict on the key answers "invalid"; an outage or rate
    // limit says nothing about it and must not send the user to replace it.
    match resp {
        Ok(mut r) => {
            let json: Value = crate::http::read_json(r.body_mut())?;
            match int_of(&json["error"]) {
                None => Ok(true),
                Some(11 | 16 | 29) => {
                    anyhow::bail!("Last.fm is temporarily unavailable. Try again later.")
                }
                Some(_) => Ok(false),
            }
        }
        Err(ureq::Error::StatusCode(400 | 403)) => Ok(false),
        Err(ureq::Error::StatusCode(code)) => {
            anyhow::bail!("Last.fm returned HTTP {code}. Try again later.")
        }
        Err(e) => Err(e.into()),
    }
}

fn cached_or_fetch(
    db: &Db,
    cache_key: &str,
    api_key: &str,
    params: &[(&str, &str)],
) -> Result<Option<Value>> {
    cached_response(db, cache_key, || fetch(api_key, params))
}

/// How long a name Last.fm does not know is remembered as unknown. Shorter
/// than the cache of real answers, so a page created later still turns up.
const NOT_FOUND_TTL_SECS: i64 = 7 * 86_400;

fn cached_response(
    db: &Db,
    cache_key: &str,
    fetch: impl FnOnce() -> Result<Option<Value>>,
) -> Result<Option<Value>> {
    // Credits look up every name in a track's tags, and many of those have no
    // Last.fm page; without this they were fetched again on every play.
    let not_found_key = format!("not-found\u{1f}{cache_key}");
    if db
        .lastfm_cache_get(&not_found_key, NOT_FOUND_TTL_SECS)?
        .is_some()
    {
        return Ok(None);
    }
    if let Some(raw) = db.lastfm_cache_get(cache_key, CACHE_TTL_SECS)? {
        if let Ok(json) = serde_json::from_str::<Value>(&raw) {
            // Older versions cached every API failure as null. Those entries
            // cannot distinguish outages from missing artists, so retry them.
            if json.is_object() && json.get("error").is_none() {
                return Ok(Some(json));
            }
        }
    }
    let Some(json) = fetch()? else {
        return Ok(None);
    };
    // API errors include invalid keys and transient failures even on HTTP 200.
    // Only successful metadata belongs in the month-long cache; "not found"
    // (error 6) gets its own shorter one.
    if !json.is_object() || json.get("error").is_some() {
        if int_of(&json["error"]) == Some(6) {
            db.lastfm_cache_put(&not_found_key, "null")?;
        }
        return Ok(None);
    }
    db.lastfm_cache_put(cache_key, &serde_json::to_string(&json)?)?;
    Ok(Some(json))
}

/// Every Last.fm request from this app shares one modest rate: Now Playing
/// credits look up several names per track change, and quick skipping sent
/// bursts past Last.fm's guideline of about five requests a second, which
/// risks the key being suspended. This runs only on blocking workers, never
/// the audio or UI thread.
pub(crate) fn pace_requests() {
    #[cfg(not(test))]
    {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);
        let mut last = LAST_REQUEST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = *last {
            std::thread::sleep(Duration::from_millis(350).saturating_sub(previous.elapsed()));
        }
        *last = Some(Instant::now());
    }
}

fn fetch(api_key: &str, params: &[(&str, &str)]) -> Result<Option<Value>> {
    pace_requests();
    let mut req = ureq::get(API_ROOT)
        .query("api_key", api_key)
        .query("format", "json")
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build();
    for (k, v) in params {
        req = req.query(k, v);
    }
    let mut resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(code)) => {
            log::warn!("last.fm returned HTTP {code}");
            return Ok(None);
        }
        Err(e) => {
            log::warn!("last.fm request failed: {e}");
            return Ok(None);
        }
    };
    let json: Value = crate::http::read_json(resp.body_mut())?;
    Ok(Some(json))
}

fn str_of(v: &Value) -> Option<String> {
    v.as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn int_of(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn names_of(v: &Value) -> Vec<String> {
    if let Some(arr) = v.as_array() {
        arr.iter().filter_map(|t| str_of(&t["name"])).collect()
    } else {
        str_of(&v["name"]).into_iter().collect()
    }
}

/// Last.fm bios end with a boilerplate trailer, e.g.
/// `<a href=...>Read more on Last.fm</a>. User-contributed text is available
/// under the Creative Commons By-SA License; additional terms may apply.`
/// Drop tags and everything from that trailer on for plain-text display.
fn strip_lastfm_link(s: String) -> String {
    let no_tags = regex::Regex::new("<[^>]+>")
        .map(|re| re.replace_all(&s, "").to_string())
        .unwrap_or(s);
    let trimmed = match no_tags.find("Read more on Last.fm") {
        Some(idx) => &no_tags[..idx],
        None => &no_tags,
    };
    trimmed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{cached_response, names_of, strip_lastfm_link, CACHE_TTL_SECS};
    use crate::db::Db;
    use serde_json::json;

    #[test]
    fn errors_are_retried_and_legacy_negative_cache_does_not_hide_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("cache.db")).unwrap();
        for code in [6, 10, 11, 16, 26, 29] {
            let key = format!("artist-{code}");
            let result = cached_response(&db, &key, || Ok(Some(json!({"error": code})))).unwrap();
            assert!(result.is_none());
            assert!(db.lastfm_cache_get(&key, CACHE_TTL_SECS).unwrap().is_none());
        }
        db.lastfm_cache_put("artist", "null").unwrap();
        let success = json!({"artist": {"name": "Recovered"}});
        assert_eq!(
            cached_response(&db, "artist", || Ok(Some(success.clone()))).unwrap(),
            Some(success.clone())
        );
        assert_eq!(
            cached_response(&db, "artist", || panic!("positive cache missed")).unwrap(),
            Some(success)
        );
    }

    /// Only "not found" is remembered, and only that one key: an outage
    /// (above) or another name must still be fetched.
    #[test]
    fn an_unknown_name_is_not_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("cache.db")).unwrap();
        let missing = cached_response(&db, "nobody", || Ok(Some(json!({"error": 6})))).unwrap();
        assert!(missing.is_none());
        let again = cached_response(&db, "nobody", || panic!("not-found cache missed")).unwrap();
        assert!(again.is_none());
        let other = json!({"artist": {"name": "Somebody"}});
        assert_eq!(
            cached_response(&db, "somebody", || Ok(Some(other.clone()))).unwrap(),
            Some(other)
        );
    }

    #[test]
    fn names_accept_lastfm_arrays_and_singletons() {
        assert_eq!(
            names_of(&json!([{ "name": "rock" }, { "name": "jazz" }])),
            ["rock", "jazz"]
        );
        assert_eq!(names_of(&json!({ "name": "rock" })), ["rock"]);
        assert!(names_of(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn strips_link_and_cc_trailer() {
        let raw = "A great band. <a href=\"https://www.last.fm/music/Band\">\
            Read more on Last.fm</a>. User-contributed text is available under \
            the Creative Commons By-SA License; additional terms may apply."
            .to_string();
        assert_eq!(strip_lastfm_link(raw), "A great band.");
    }

    #[test]
    fn leaves_plain_bio_untouched() {
        let raw = "Just a description with no trailer.".to_string();
        assert_eq!(strip_lastfm_link(raw.clone()), raw);
    }
}
