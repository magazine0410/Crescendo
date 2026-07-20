use crate::state::AppState;
use crescendo_core::lastfm::{AlbumInfo, ArtistInfo};
use std::collections::HashMap;
use tauri::{AppHandle, Manager, State, WebviewUrl};
use tokio::sync::Semaphore;

/// Bound concurrent waveform decodes (each fully decodes a track). Skipping
/// tracks quickly could otherwise stack several CPU-heavy decodes at once.
static WAVEFORM_PERMITS: Semaphore = Semaphore::const_new(2);

#[tauri::command]
pub async fn prefs_get_all(state: State<'_, AppState>) -> Result<HashMap<String, String>, String> {
    state.db.prefs_get_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn prefs_set(
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    state.db.prefs_set(&key, &value).map_err(|e| e.to_string())
}

/// Loudness-waveform peaks (0..=255) for a track; generated and cached on first
/// request, then served from disk. Runs off-thread so the UI never blocks.
#[tauri::command]
pub async fn waveform_get(state: State<'_, AppState>, track_id: i64) -> Result<Vec<u8>, String> {
    let _permit = WAVEFORM_PERMITS.acquire().await.map_err(|e| e.to_string())?;
    let db = state.db.clone();
    let wf = state.waveform.clone();
    tauri::async_runtime::spawn_blocking(move || {
        wf.get_or_generate(&db, track_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn api_key(state: &State<'_, AppState>) -> String {
    state.db.prefs_get("lastfm_api_key").ok().flatten().unwrap_or_default()
}

#[tauri::command]
pub async fn lastfm_artist_info(
    state: State<'_, AppState>,
    artist: String,
) -> Result<Option<ArtistInfo>, String> {
    let key = api_key(&state);
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crescendo_core::lastfm::artist_info(&db, &key, &artist).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn lastfm_album_info(
    state: State<'_, AppState>,
    artist: String,
    album: String,
) -> Result<Option<AlbumInfo>, String> {
    let key = api_key(&state);
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crescendo_core::lastfm::album_info(&db, &key, &artist, &album).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn lastfm_test_key(api_key: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crescendo_core::lastfm::test_key(&api_key).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open (or focus) the standalone cover-art window.
#[tauri::command]
pub async fn open_cover_window(
    app: AppHandle,
    album_id: Option<i64>,
    track_id: Option<i64>,
) -> Result<(), String> {
    let query = match (album_id, track_id) {
        (Some(a), _) => format!("album={a}"),
        (None, Some(t)) => format!("track={t}"),
        _ => return Err("nothing to show".into()),
    };
    let url = format!("/cover?{query}");
    if let Some(win) = app.get_webview_window("cover") {
        // Reuse the window, navigate it to the new art.
        win.eval(&format!("window.location.href = '{url}'")).map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(&app, "cover", WebviewUrl::App(url.into()))
        .title("Cover art")
        .inner_size(600.0, 600.0)
        .min_inner_size(200.0, 200.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}
