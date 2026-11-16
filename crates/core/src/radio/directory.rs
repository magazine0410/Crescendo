//! Client for Radio Browser (radio-browser.info), a free community directory
//! of internet radio stations.
//!
//! Its API runs on several mirrors listed at `all.api.radio-browser.info`; one
//! is picked at random and kept until a request to it fails. Requests carry a
//! descriptive User-Agent, as the service asks, and nothing else about the
//! user.

use super::NewStation;
use crate::http::read_json;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

const SERVERS_URL: &str = "https://all.api.radio-browser.info/json/servers";
/// Used when the server list itself cannot be fetched.
const FALLBACK_SERVER: &str = "de1.api.radio-browser.info";
const TIMEOUT: Duration = Duration::from_secs(15);
pub const PAGE_SIZE: usize = 100;

/// One station in a directory listing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DirectoryStation {
    pub uuid: String,
    pub name: String,
    pub stream_url: String,
    pub homepage: Option<String>,
    #[serde(skip)]
    pub logo_url: Option<String>,
    /// Key for `crescendo-art://…/logo/{key}`, set by the caller that
    /// registered the logo.
    pub logo: Option<String>,
    pub tags: Option<String>,
    pub country: Option<String>,
    pub countrycode: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i64>,
}

impl DirectoryStation {
    pub fn to_new_station(&self) -> NewStation {
        NewStation {
            name: self.name.clone(),
            stream_url: self.stream_url.clone(),
            homepage: self.homepage.clone(),
            logo_url: self.logo_url.clone(),
            tags: self.tags.clone(),
            country: self.country.clone(),
            codec: self.codec.clone(),
            bitrate: self.bitrate,
            uuid: Some(self.uuid.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Country {
    pub code: String,
    pub name: String,
    pub stations: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tag {
    pub name: String,
    pub stations: i64,
}

/// What to search for. Empty fields do not narrow the search.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Search {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub countrycode: String,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Deserialize)]
struct RawServer {
    name: String,
}

#[derive(Deserialize)]
struct RawStation {
    stationuuid: String,
    name: String,
    url: String,
    #[serde(default)]
    url_resolved: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    favicon: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    countrycode: String,
    #[serde(default)]
    codec: String,
    #[serde(default)]
    bitrate: i64,
}

#[derive(Deserialize)]
struct RawCountry {
    name: String,
    iso_3166_1: String,
    stationcount: i64,
}

#[derive(Deserialize)]
struct RawTag {
    name: String,
    stationcount: i64,
}

fn some(text: String) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Stations whose stream is not http(s) are dropped: they cannot be played.
fn convert(raw: RawStation) -> Option<DirectoryStation> {
    let stream_url = some(raw.url_resolved).unwrap_or(raw.url);
    super::check_stream_url(&stream_url).ok()?;
    Some(DirectoryStation {
        uuid: raw.stationuuid,
        name: some(raw.name).unwrap_or_else(|| "Unnamed station".into()),
        stream_url,
        homepage: some(raw.homepage),
        logo_url: some(raw.favicon).filter(|u| super::check_stream_url(u).is_ok()),
        logo: None,
        tags: some(raw.tags),
        country: some(raw.country),
        countrycode: some(raw.countrycode),
        codec: some(raw.codec),
        bitrate: (raw.bitrate > 0).then_some(raw.bitrate),
    })
}

pub struct Directory {
    user_agent: String,
    server: Mutex<Option<String>>,
}

impl Directory {
    /// `user_agent` names the application, e.g. "Crescendo/1.0.4".
    pub fn new(user_agent: impl Into<String>) -> Self {
        Self {
            user_agent: user_agent.into(),
            server: Mutex::new(None),
        }
    }

    fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(self.user_agent.as_str())
            .build()
            .into()
    }

    fn pick_server(&self) -> String {
        if let Some(server) = self.server.lock().unwrap().clone() {
            return server;
        }
        let listed = self
            .agent()
            .get(SERVERS_URL)
            .call()
            .ok()
            .and_then(|mut r| read_json::<Vec<RawServer>>(r.body_mut()).ok())
            .unwrap_or_default();
        let mut names: Vec<String> = listed.into_iter().map(|s| s.name).collect();
        names.sort();
        names.dedup();
        let server = if names.is_empty() {
            FALLBACK_SERVER.to_string()
        } else {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(0);
            names[nanos % names.len()].clone()
        };
        *self.server.lock().unwrap() = Some(server.clone());
        server
    }

    /// GET a path on the chosen mirror. A failure forgets the mirror and tries
    /// once more on another.
    fn get<T: serde::de::DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let mut last_error = None;
        for _ in 0..2 {
            let server = self.pick_server();
            let mut url = url::Url::parse(&format!("https://{server}{path}"))?;
            url.query_pairs_mut().extend_pairs(query);
            match self
                .agent()
                .get(url.as_str())
                .call()
                .map_err(anyhow::Error::from)
                .and_then(|mut r| read_json::<T>(r.body_mut()))
            {
                Ok(value) => return Ok(value),
                Err(error) => {
                    *self.server.lock().unwrap() = None;
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("no directory server answered")))
            .context("the Radio Browser directory did not answer")
    }

    /// Working stations matching `search`, most played first.
    pub fn search(&self, search: &Search) -> Result<Vec<DirectoryStation>> {
        let offset = search.offset.to_string();
        let limit = PAGE_SIZE.to_string();
        let mut query = vec![
            ("hidebroken", "true"),
            ("order", "clickcount"),
            ("reverse", "true"),
            ("limit", limit.as_str()),
            ("offset", offset.as_str()),
        ];
        for (key, value) in [
            ("name", search.name.trim()),
            ("tag", search.tag.trim()),
            ("countrycode", search.countrycode.trim()),
        ] {
            if !value.is_empty() {
                query.push((key, value));
            }
        }
        if !search.tag.trim().is_empty() {
            query.push(("tagExact", "true"));
        }
        let raw: Vec<RawStation> = self.get("/json/stations/search", &query)?;
        Ok(raw.into_iter().filter_map(convert).collect())
    }

    /// One station by its directory id.
    pub fn station(&self, uuid: &str) -> Result<DirectoryStation> {
        let raw: Vec<RawStation> = self.get(&format!("/json/stations/byuuid/{uuid}"), &[])?;
        raw.into_iter()
            .find_map(convert)
            .ok_or_else(|| anyhow!("the directory has no playable station {uuid}"))
    }

    /// Countries that have working stations, most stations first.
    pub fn countries(&self) -> Result<Vec<Country>> {
        let raw: Vec<RawCountry> = self.get(
            "/json/countries",
            &[
                ("order", "stationcount"),
                ("reverse", "true"),
                ("hidebroken", "true"),
            ],
        )?;
        Ok(raw
            .into_iter()
            .filter(|c| c.stationcount > 0 && c.iso_3166_1.len() == 2)
            .map(|c| Country {
                code: c.iso_3166_1,
                name: c.name,
                stations: c.stationcount,
            })
            .collect())
    }

    /// The most used genre tags.
    pub fn tags(&self) -> Result<Vec<Tag>> {
        let raw: Vec<RawTag> = self.get(
            "/json/tags",
            &[
                ("order", "stationcount"),
                ("reverse", "true"),
                ("hidebroken", "true"),
                ("limit", "300"),
            ],
        )?;
        Ok(raw
            .into_iter()
            .filter(|t| t.stationcount > 0 && !t.name.trim().is_empty())
            .map(|t| Tag {
                name: t.name,
                stations: t.stationcount,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(url: &str, resolved: &str, favicon: &str) -> RawStation {
        RawStation {
            stationuuid: "u".into(),
            name: " Gensokyo Radio ".into(),
            url: url.into(),
            url_resolved: resolved.into(),
            homepage: "".into(),
            favicon: favicon.into(),
            tags: "anime,touhou".into(),
            country: "Japan".into(),
            countrycode: "JP".into(),
            codec: "MP3".into(),
            bitrate: 0,
        }
    }

    #[test]
    fn listings_prefer_the_resolved_stream_and_drop_unplayable_ones() {
        let s = convert(raw("https://x/a.m3u", "https://x/3", "https://x/f.ico")).unwrap();
        assert_eq!(s.stream_url, "https://x/3");
        assert_eq!(s.name, "Gensokyo Radio");
        assert_eq!(s.logo_url.as_deref(), Some("https://x/f.ico"));
        assert_eq!(s.homepage, None);
        assert_eq!(s.bitrate, None);
        let s = convert(raw("https://x/live", "", "data:image/png;base64,AA")).unwrap();
        assert_eq!(s.stream_url, "https://x/live");
        assert_eq!(s.logo_url, None, "only http(s) logos are fetched");
        assert!(convert(raw("rtmp://x/live", "", "")).is_none());
    }
}
