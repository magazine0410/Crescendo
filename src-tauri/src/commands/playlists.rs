use crate::state::AppState;
use crescendo_core::db::playlists::PlaylistInfo;
use crescendo_core::m3u::M3uImportResult;
use crescendo_core::model::Track;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub async fn playlist_list(state: State<'_, AppState>) -> Result<Vec<PlaylistInfo>, String> {
    state.db.playlist_list().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn playlist_create(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> Result<i64, String> {
    let id = state.db.playlist_create(&name).map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", id);
    Ok(id)
}

#[tauri::command]
pub async fn playlist_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    state.db.playlist_rename(id, &name).map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", id);
    Ok(())
}

#[tauri::command]
pub async fn playlist_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    state.db.playlist_delete(id).map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", id);
    Ok(())
}

#[tauri::command]
pub async fn playlist_tracks(state: State<'_, AppState>, id: i64) -> Result<Vec<Track>, String> {
    state.db.playlist_tracks(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn track_playlist_ids(
    state: State<'_, AppState>,
    track_id: i64,
) -> Result<Vec<i64>, String> {
    state.db.playlist_ids_with_track(track_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn playlist_add_tracks(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    track_ids: Vec<i64>,
) -> Result<(), String> {
    state.db.playlist_add_tracks(id, &track_ids).map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", id);
    Ok(())
}

#[tauri::command]
pub async fn playlist_remove_tracks(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    positions: Vec<i64>,
) -> Result<(), String> {
    state.db.playlist_remove_tracks(id, &positions).map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", id);
    Ok(())
}

#[tauri::command]
pub async fn playlist_reorder(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    from: Vec<i64>,
    to: i64,
) -> Result<(), String> {
    state.db.playlist_reorder(id, &from, to).map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", id);
    Ok(())
}

#[tauri::command]
pub async fn playlist_import_m3u(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<M3uImportResult, String> {
    let result =
        crescendo_core::m3u::import(&state.db, &PathBuf::from(path)).map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", result.playlist_id);
    Ok(result)
}
