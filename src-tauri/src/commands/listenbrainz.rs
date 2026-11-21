use crate::state::AppState;
use crescendo_core::{
    artistinfo::profile,
    db::history::HistoryEntry,
    listenbrainz::{scrobble::ScrobbleStatus, Endpoints},
    songlink::{self, ListenbrainzLinks},
};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Semaphore;

static LOOKUPS: Semaphore = Semaphore::const_new(2);
/// Plays on one page of Song history.
const MAX_LINKS: usize = 100;

#[tauri::command]
pub async fn listenbrainz_status(state: State<'_, AppState>) -> Result<ScrobbleStatus, String> {
    state.scrobbler.status().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn listenbrainz_connect(state: State<'_, AppState>, token: String) -> Result<(), String> {
    let service = state.scrobbler.clone();
    tauri::async_runtime::spawn_blocking(move || service.connect(token))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn listenbrainz_set_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let service = state.scrobbler.clone();
    tauri::async_runtime::spawn_blocking(move || service.set_enabled(enabled))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn listenbrainz_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    let service = state.scrobbler.clone();
    tauri::async_runtime::spawn_blocking(move || service.disconnect())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn song_history(
    state: State<'_, AppState>,
    before: Option<i64>,
) -> Result<Vec<HistoryEntry>, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || db.song_history(before, 50))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// The ListenBrainz pages of the tracks it matches, for the plays Song
/// history shows. Without a connected account there are none.
#[tauri::command]
pub async fn listenbrainz_links(
    state: State<'_, AppState>,
    track_ids: Vec<i64>,
) -> Result<ListenbrainzLinks, String> {
    let Some(token) = state.scrobbler.token() else {
        return Ok(ListenbrainzLinks::default());
    };
    if track_ids.len() > MAX_LINKS {
        return Err("too many songs in one request".into());
    }
    let _permit = LOOKUPS.acquire().await.map_err(|e| e.to_string())?;
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<ListenbrainzLinks> {
        let tracks = db.tracks_by_ids(&track_ids)?;
        Ok(songlink::listenbrainz_links(
            &db,
            &Endpoints::default(),
            &token,
            &tracks,
        ))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Opens a page Crescendo itself linked to: a song's page on Last.fm or
/// ListenBrainz, or an artist's page on one of the profile sites. Anything
/// else the webview passes is refused.
#[tauri::command]
pub async fn open_info_link(app: AppHandle, url: String) -> Result<(), String> {
    let url = songlink::valid_link(&url)
        .or_else(|| profile::valid_link(&url))
        .ok_or("That address is not one Crescendo links to.")?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}
