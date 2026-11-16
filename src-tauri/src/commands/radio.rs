//! Internet radio commands: saved stations, the Radio Browser directory, and
//! playing a station. The logic lives in `crescendo_core::radio`.

use crate::player::Cmd;
use crate::state::AppState;
use crescendo_core::lyrics::LyricsResult;
use crescendo_core::radio::directory::{Country, Directory, DirectoryStation, Search, Tag};
use crescendo_core::radio::logo::LogoStore;
use crescendo_core::radio::{NewStation, Station};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Radio state shared by the commands, the player and the art protocol.
pub struct Radio {
    pub directory: Directory,
    pub logos: Arc<LogoStore>,
    /// Directory stations the last searches returned, by uuid, so saving or
    /// playing one needs no second request and takes nothing from the webview
    /// but its id.
    listed: Mutex<HashMap<String, DirectoryStation>>,
    countries: Mutex<Option<Vec<Country>>>,
    tags: Mutex<Option<Vec<Tag>>>,
}

/// Listings kept for saving and playing; older ones are fetched again.
const MAX_LISTED: usize = 2000;

impl Radio {
    pub fn new(user_agent: String, logos: Arc<LogoStore>) -> Self {
        Self {
            directory: Directory::new(user_agent),
            logos,
            listed: Mutex::new(HashMap::new()),
            countries: Mutex::new(None),
            tags: Mutex::new(None),
        }
    }

    fn with_logo(&self, mut station: Station) -> Station {
        station.logo = station
            .logo_url
            .as_deref()
            .map(|url| self.logos.register(url));
        station
    }

    fn remember(&self, stations: &mut [DirectoryStation]) {
        let mut listed = self.listed.lock().unwrap();
        if listed.len() + stations.len() > MAX_LISTED {
            listed.clear();
        }
        for station in stations.iter_mut() {
            station.logo = station
                .logo_url
                .as_deref()
                .map(|url| self.logos.register(url));
            listed.insert(station.uuid.clone(), station.clone());
        }
    }

    /// A directory station by id: from the last listings, else asked for.
    fn directory_station(&self, uuid: &str) -> anyhow::Result<DirectoryStation> {
        // It goes into a URL path, and Radio Browser ids are UUIDs.
        if uuid.is_empty()
            || uuid.len() > 64
            || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        {
            anyhow::bail!("not a directory station id");
        }
        if let Some(station) = self.listed.lock().unwrap().get(uuid).cloned() {
            return Ok(station);
        }
        let mut station = vec![self.directory.station(uuid)?];
        self.remember(&mut station);
        Ok(station.remove(0))
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Saved stations in list order.
#[tauri::command]
pub async fn radio_stations(state: State<'_, AppState>) -> Result<Vec<Station>, String> {
    let db = state.db.clone();
    let radio = state.radio.clone();
    blocking(move || {
        Ok(db
            .stations()?
            .into_iter()
            .map(|s| radio.with_logo(s))
            .collect())
    })
    .await
}

/// Search the directory. Empty fields do not narrow it; an empty search
/// lists the most played stations.
#[tauri::command]
pub async fn radio_search(
    state: State<'_, AppState>,
    search: Search,
) -> Result<Vec<DirectoryStation>, String> {
    let radio = state.radio.clone();
    blocking(move || {
        let mut found = radio.directory.search(&search)?;
        radio.remember(&mut found);
        Ok(found)
    })
    .await
}

#[tauri::command]
pub async fn radio_countries(state: State<'_, AppState>) -> Result<Vec<Country>, String> {
    let radio = state.radio.clone();
    blocking(move || {
        if let Some(countries) = radio.countries.lock().unwrap().clone() {
            return Ok(countries);
        }
        let countries = radio.directory.countries()?;
        *radio.countries.lock().unwrap() = Some(countries.clone());
        Ok(countries)
    })
    .await
}

#[tauri::command]
pub async fn radio_tags(state: State<'_, AppState>) -> Result<Vec<Tag>, String> {
    let radio = state.radio.clone();
    blocking(move || {
        if let Some(tags) = radio.tags.lock().unwrap().clone() {
            return Ok(tags);
        }
        let tags = radio.directory.tags()?;
        *radio.tags.lock().unwrap() = Some(tags.clone());
        Ok(tags)
    })
    .await
}

/// Save a directory station. Saving one twice returns the saved one.
#[tauri::command]
pub async fn radio_save(state: State<'_, AppState>, uuid: String) -> Result<Station, String> {
    let db = state.db.clone();
    let radio = state.radio.clone();
    blocking(move || {
        let found = radio.directory_station(&uuid)?;
        let id = db.station_add(&found.to_new_station())?;
        let station = db
            .station(id)?
            .ok_or_else(|| anyhow::anyhow!("the station was not saved"))?;
        Ok(radio.with_logo(station))
    })
    .await
}

/// Save a station from an address typed by the user. A .pls or .m3u
/// playlist is opened and its first stream saved.
#[tauri::command]
pub async fn radio_add_url(
    state: State<'_, AppState>,
    name: String,
    url: String,
) -> Result<Station, String> {
    let db = state.db.clone();
    let radio = state.radio.clone();
    blocking(move || {
        let stream_url = crescendo_core::radio::resolve_stream_url(&url)?;
        let id = db.station_add(&NewStation {
            name,
            stream_url,
            ..Default::default()
        })?;
        let station = db
            .station(id)?
            .ok_or_else(|| anyhow::anyhow!("the station was not saved"))?;
        Ok(radio.with_logo(station))
    })
    .await
}

#[tauri::command]
pub async fn radio_rename(state: State<'_, AppState>, id: i64, name: String) -> Result<(), String> {
    let db = state.db.clone();
    blocking(move || db.station_rename(id, &name)).await
}

#[tauri::command]
pub async fn radio_remove(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let db = state.db.clone();
    blocking(move || db.station_remove(id)).await
}

/// Play a saved station (`id`) or a directory listing (`uuid`).
#[tauri::command]
pub async fn player_play_station(
    state: State<'_, AppState>,
    id: Option<i64>,
    uuid: Option<String>,
) -> Result<(), String> {
    let db = state.db.clone();
    let radio = state.radio.clone();
    let station = blocking(move || match (id, uuid) {
        (Some(id), _) => {
            let station = db
                .station(id)?
                .ok_or_else(|| anyhow::anyhow!("station {id} is not saved"))?;
            Ok(radio.with_logo(station))
        }
        (None, Some(uuid)) => {
            let found = radio.directory_station(&uuid)?;
            let new = found.to_new_station();
            Ok(Station {
                id: 0,
                name: new.name,
                stream_url: new.stream_url,
                homepage: new.homepage,
                logo_url: new.logo_url,
                logo: found.logo,
                tags: new.tags,
                country: new.country,
                codec: new.codec,
                bitrate: new.bitrate,
                uuid: new.uuid,
            })
        }
        (None, None) => anyhow::bail!("no station given"),
    })
    .await?;
    state.player.send(Cmd::PlayStation(Box::new(station)))
}

/// Lyrics for the song a station announces (online sources only).
#[tauri::command]
pub async fn lyrics_for_song(
    state: State<'_, AppState>,
    artist: String,
    title: String,
) -> Result<Option<LyricsResult>, String> {
    let db = state.db.clone();
    blocking(move || crescendo_core::lyrics::for_song(&db, &artist, &title)).await
}
