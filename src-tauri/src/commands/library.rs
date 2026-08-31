use crate::state::AppState;
use crescendo_core::model::{
    Album, BrowseMode, BrowseValue, LibraryStats, Track, TrackGroup, TrackSort,
};
use crescendo_core::scanner::{scan, ScanOptions};
use crescendo_core::search::SearchFilter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Clears the process-wide scan latch on every exit path, including unwinding
/// from a scanner panic and failure to create the scanner thread (which drops
/// the still-captured closure).
struct ScanningGuard(Arc<AtomicBool>);

impl Drop for ScanningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn filter_for(query: Option<&str>) -> Result<Option<SearchFilter>, String> {
    match query {
        Some(q) if !q.trim().is_empty() => crescendo_core::search::compile(q)
            .map(Some)
            .map_err(|e| e.to_string()),
        _ => Ok(None),
    }
}

#[tauri::command]
pub async fn scan_start(
    app: AppHandle,
    state: State<'_, AppState>,
    full: bool,
) -> Result<(), String> {
    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("a scan is already running".into());
    }
    state.scan_cancel.store(false, Ordering::SeqCst);

    let db = state.db.clone();
    let scanning = state.scanning.clone();
    let art = state.art.clone();
    let cancel = state.scan_cancel.clone();
    let root = db
        .prefs_get("library_path")
        .ok()
        .flatten()
        .unwrap_or_else(|| "/mnt/Mythic_Meme/Music/".to_string());
    let threads: usize = db
        .prefs_get("scan_threads")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let scanning_guard = ScanningGuard(scanning);

    std::thread::Builder::new()
        .name("scanner".into())
        .spawn(move || {
            let scanning_guard = scanning_guard;
            let opts = ScanOptions {
                root: root.into(),
                threads,
                full,
            };
            let progress_app = app.clone();
            let outcome = scan(
                &db,
                &opts,
                &move |p| {
                    let _ = progress_app.emit("scan:progress", &p);
                },
                &cancel,
            );
            if outcome.is_ok() {
                if let Err(e) = art.invalidate_stale(&db) {
                    log::warn!("cover cache invalidation failed after scan: {e}");
                }
            }
            // Match the old event ordering: observers of done/error should
            // already be able to start another scan.
            drop(scanning_guard);
            match outcome {
                Ok(p) => {
                    let _ = app.emit("scan:done", &p);
                    let _ = app.emit("library:changed", ());
                }
                Err(e) => {
                    log::error!("scan failed: {e}");
                    let _ = app.emit("scan:error", e.to_string());
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn scan_cancel(state: State<'_, AppState>) -> Result<(), String> {
    state.scan_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ScanningGuard;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn scanning_guard_clears_the_latch_when_dropped() {
        let scanning = Arc::new(AtomicBool::new(true));
        {
            let _guard = ScanningGuard(scanning.clone());
        }
        assert!(!scanning.load(Ordering::SeqCst));
    }
}

#[tauri::command]
pub async fn library_stats(state: State<'_, AppState>) -> Result<LibraryStats, String> {
    let stats = state.db.library_stats().map_err(|e| e.to_string())?;
    log::info!("library_stats served to webview: {} tracks", stats.tracks);
    Ok(stats)
}

#[tauri::command]
pub async fn browse_values(
    state: State<'_, AppState>,
    mode: BrowseMode,
    query: Option<String>,
    collapse: bool,
) -> Result<Vec<BrowseValue>, String> {
    let filter = filter_for(query.as_deref())?;
    state
        .db
        .browse_values(mode, filter.as_ref(), collapse)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn browse_albums(
    state: State<'_, AppState>,
    mode: BrowseMode,
    values: Option<Vec<String>>,
    query: Option<String>,
    collapse: bool,
) -> Result<Vec<Album>, String> {
    let filter = filter_for(query.as_deref())?;
    state
        .db
        .browse_albums(mode, &values.unwrap_or_default(), filter.as_ref(), collapse)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn album_tracks(state: State<'_, AppState>, album_id: i64) -> Result<Vec<Track>, String> {
    state.db.album_tracks(album_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn track_count(
    state: State<'_, AppState>,
    query: Option<String>,
    mode: BrowseMode,
    values: Option<Vec<String>>,
    collapse: bool,
) -> Result<i64, String> {
    let filter = filter_for(query.as_deref())?;
    state
        .db
        .track_count(filter.as_ref(), mode, &values.unwrap_or_default(), collapse)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn tracks_range(
    state: State<'_, AppState>,
    query: Option<String>,
    mode: BrowseMode,
    values: Option<Vec<String>>,
    collapse: bool,
    sort: Option<TrackSort>,
    offset: i64,
    limit: i64,
) -> Result<Vec<Track>, String> {
    let filter = filter_for(query.as_deref())?;
    state
        .db
        .tracks_range(
            filter.as_ref(),
            mode,
            &values.unwrap_or_default(),
            collapse,
            sort,
            offset,
            limit.clamp(1, 2000),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn track_index(
    state: State<'_, AppState>,
    query: Option<String>,
    mode: BrowseMode,
    values: Option<Vec<String>>,
    collapse: bool,
    sort: Option<TrackSort>,
    track_id: i64,
) -> Result<Option<i64>, String> {
    let filter = filter_for(query.as_deref())?;
    state
        .db
        .track_index(
            filter.as_ref(),
            mode,
            &values.unwrap_or_default(),
            collapse,
            sort,
            track_id,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn track_groups(
    state: State<'_, AppState>,
    query: Option<String>,
    mode: BrowseMode,
    values: Option<Vec<String>>,
    collapse: bool,
    sort: Option<TrackSort>,
) -> Result<Vec<TrackGroup>, String> {
    let filter = filter_for(query.as_deref())?;
    state
        .db
        .track_groups(
            filter.as_ref(),
            mode,
            &values.unwrap_or_default(),
            collapse,
            sort,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn album_by_id(
    state: State<'_, AppState>,
    album_id: i64,
) -> Result<Option<Album>, String> {
    state.db.album_by_id(album_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn search_classify(query: String) -> Result<crescendo_core::search::QueryKind, String> {
    Ok(crescendo_core::search::classify(&query))
}
