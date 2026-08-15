use crate::state::AppState;
use crescendo_core::db::collections::CollectionInfo;
use crescendo_core::db::playlists::{PlaylistInfo, PlaylistTracks};
use crescendo_core::import::DropImportResult;
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
    state
        .db
        .playlist_rename(id, &name)
        .map_err(|e| e.to_string())?;
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

/// Persist the sidebar order of the playlists. Deliberately emits no
/// `playlist:changed`: the caller already shows the new order, and a refetch
/// would only make the sidebar rebuild what it just drew.
#[tauri::command]
pub async fn playlists_set_order(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    state
        .db
        .playlists_set_order(&ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn playlist_tracks(state: State<'_, AppState>, id: i64) -> Result<Vec<Track>, String> {
    state.db.playlist_tracks(id).map_err(|e| e.to_string())
}

/// Tracks of every selected playlist, grouped per playlist so the view keeps
/// track of which row belongs where.
#[tauri::command]
pub async fn playlist_tracks_multi(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<Vec<PlaylistTracks>, String> {
    state
        .db
        .playlist_tracks_multi(&ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn track_playlist_ids(
    state: State<'_, AppState>,
    track_id: i64,
) -> Result<Vec<i64>, String> {
    state
        .db
        .playlist_ids_with_track(track_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn playlist_add_tracks(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    track_ids: Vec<i64>,
) -> Result<(), String> {
    state
        .db
        .playlist_add_tracks(id, &track_ids)
        .map_err(|e| e.to_string())?;
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
    state
        .db
        .playlist_remove_tracks(id, &positions)
        .map_err(|e| e.to_string())?;
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
    state
        .db
        .playlist_reorder(id, &from, to)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", id);
    Ok(())
}

/// Append dropped files and folders to a playlist. Tag reading can take a
/// moment on a folder off an HDD, so it runs off the command thread.
#[tauri::command]
pub async fn playlist_add_paths(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    paths: Vec<String>,
) -> Result<DropImportResult, String> {
    let db = state.db.clone();
    let art = state.art.clone();
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = crescendo_core::import::add_paths_to_playlist(&db, id, &paths)
            .map_err(|e| e.to_string())?;
        if result.imported > 0 {
            if let Err(e) = art.invalidate_stale(&db) {
                log::warn!("cover cache invalidation failed after import: {e}");
            }
        }
        Ok::<_, String>(result)
    })
    .await
    .map_err(|e| e.to_string())??;

    let _ = app.emit("playlist:changed", id);
    // Only a drop that pulled new files into the library moves the counts.
    if result.imported > 0 {
        let _ = app.emit("library:changed", ());
    }
    Ok(result)
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

#[tauri::command]
pub async fn collection_list(state: State<'_, AppState>) -> Result<Vec<CollectionInfo>, String> {
    state.db.collection_list().map_err(|e| e.to_string())
}

/// Put playlists into the collection named `name`, creating it when it is new.
/// A blank name takes them out of whatever collection they were in.
#[tauri::command]
pub async fn collection_set(
    app: AppHandle,
    state: State<'_, AppState>,
    playlist_ids: Vec<i64>,
    name: String,
) -> Result<Option<i64>, String> {
    let id = state
        .db
        .collection_set(&playlist_ids, &name)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", ());
    Ok(id)
}

#[tauri::command]
pub async fn collection_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    state
        .db
        .collection_rename(id, &name)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", ());
    Ok(())
}

#[tauri::command]
pub async fn collection_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    state.db.collection_delete(id).map_err(|e| e.to_string())?;
    let _ = app.emit("playlist:changed", ());
    Ok(())
}

/// Fold or unfold a collection. No event: the sidebar already drew it, and a
/// refetch would only rebuild the list under the click.
#[tauri::command]
pub async fn collection_set_collapsed(
    state: State<'_, AppState>,
    id: i64,
    collapsed: bool,
) -> Result<(), String> {
    state
        .db
        .collection_set_collapsed(id, collapsed)
        .map_err(|e| e.to_string())
}
