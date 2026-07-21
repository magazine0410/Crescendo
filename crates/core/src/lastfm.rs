//! Minimal Last.fm API client (artist.getInfo / album.getInfo) with a
//! 30-day response cache in SQLite. Callers fall back to local tags when
//! this returns None (no key, offline, or no result).

use crate::db::Db;
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

const API_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";
const CACHE_TTL_SECS: i64 = 30 * 24 * 3600;

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
    let json = match cached_or_fetch(db, &cache_key, api_key, &[("method", "artist.getinfo"), ("artist", artist)])? {
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
    if api_key.is_empty() || artist.is_empty() || album.is_empty() {
        return Ok(None);
    }
    let cache_key = format!("album\u{1f}{}\u{1f}{}", artist.to_lowercase(), album.to_lowercase());
    let json = match cached_or_fetch(
        db,
        &cache_key,
        api_key,
        &[("method", "album.getinfo"), ("artist", artist), ("album", album)],
    )? {
        Some(j) => j,
        None => return Ok(None),
    };
    let a = &json["album"];
    if a.is_null() {
        return Ok(None);
    }
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

/// Quick key check for the settings dialog "Test key" button.
pub fn test_key(api_key: &str) -> Result<bool> {
    let resp = ureq::get(API_ROOT)
        .query("method", "artist.getinfo")
        .query("artist", "cher")
        .query("api_key", api_key)
        .query("format", "json")
        .timeout(std::time::Duration::from_secs(10))
        .call();
    match resp {
        Ok(r) => {
            let json: Value = r.into_json()?;
            Ok(json.get("error").is_none())
        }
        Err(ureq::Error::Status(_, _)) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn cached_or_fetch(
    db: &Db,
    cache_key: &str,
    api_key: &str,
    params: &[(&str, &str)],
) -> Result<Option<Value>> {
    if let Some(raw) = db.lastfm_cache_get(cache_key, CACHE_TTL_SECS)? {
        return Ok(serde_json::from_str(&raw).ok());
    }
    let mut req = ureq::get(API_ROOT)
        .query("api_key", api_key)
        .query("format", "json")
        .timeout(std::time::Duration::from_secs(15));
    for (k, v) in params {
        req = req.query(k, v);
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => {
            log::warn!("last.fm returned HTTP {code} for {cache_key}");
            return Ok(None);
        }
        Err(e) => {
            log::warn!("last.fm request failed: {e}");
            return Ok(None);
        }
    };
    let json: Value = resp.into_json()?;
    if json.get("error").is_some() {
        // Cache "no result" too, so unknown artists don't re-query for a month.
        db.lastfm_cache_put(cache_key, "null")?;
        return Ok(None);
    }
    db.lastfm_cache_put(cache_key, &serde_json::to_string(&json)?)?;
    Ok(Some(json))
}

fn str_of(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn int_of(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn names_of(v: &Value) -> Vec<String> {
    match v.as_array() {
        Some(arr) => arr.iter().filter_map(|t| str_of(&t["name"])).collect(),
        None => vec![],
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
    use super::strip_lastfm_link;

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
