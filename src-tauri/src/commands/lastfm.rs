use crate::state::AppState;
use crescendo_core::{
    db::history::HistoryEntry,
    lastfm::scrobble::{self, ScrobbleStatus},
};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Semaphore;

static LOOKUPS: Semaphore = Semaphore::const_new(2);

#[tauri::command]
pub async fn lastfm_status(state: State<'_, AppState>) -> Result<ScrobbleStatus, String> {
    state.scrobbler.status().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn lastfm_connect(
    app: AppHandle,
    state: State<'_, AppState>,
    api_key: String,
    secret: String,
) -> Result<(), String> {
    let service = state.scrobbler.clone();
    let url = tauri::async_runtime::spawn_blocking(move || service.connect(api_key, secret))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn lastfm_finish_connect(state: State<'_, AppState>) -> Result<(), String> {
    let service = state.scrobbler.clone();
    tauri::async_runtime::spawn_blocking(move || service.finish_connect())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn lastfm_set_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let service = state.scrobbler.clone();
    tauri::async_runtime::spawn_blocking(move || service.set_enabled(enabled))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn lastfm_disconnect(state: State<'_, AppState>) -> Result<(), String> {
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

#[tauri::command]
pub async fn lastfm_track_url(
    state: State<'_, AppState>,
    track_id: i64,
) -> Result<Option<String>, String> {
    let _permit = LOOKUPS.acquire().await.map_err(|e| e.to_string())?;
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let Some(track) = db.track_by_id(track_id)? else {
            return Ok(None);
        };
        let key = db.prefs_get("lastfm_api_key")?.unwrap_or_default();
        scrobble::track_url(&db, &key, &track)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn lastfm_open_track(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
) -> Result<(), String> {
    let url = lastfm_track_url(state, track_id)
        .await?
        .ok_or("This song was not found on Last.fm.")?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}
