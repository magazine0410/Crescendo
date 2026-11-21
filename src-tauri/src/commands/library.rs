use crate::state::AppState;
use crescendo_core::library_lock::{LibraryLock, LibraryWriter};
use crescendo_core::model::{
    Album, BrowseMode, BrowseValue, FacetSelection, LibraryStats, PlayContext, Track, TrackGroup,
    TrackSort,
};
use crescendo_core::plugins::StaleProjection;
use crescendo_core::scanner::{scan, ScanOptions};
use crescendo_core::search::SearchFilter;
use serde::Serialize;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectionCommandError {
    StaleProjection { current_revision: u64 },
    Operation { message: String },
}

#[derive(Debug, Serialize)]
pub struct BrowseValuesResult {
    pub projection_revision: u64,
    pub values: Vec<BrowseValue>,
}

pub(crate) fn projection_error(error: anyhow::Error) -> ProjectionCommandError {
    if let Some(stale) = error.downcast_ref::<StaleProjection>() {
        ProjectionCommandError::StaleProjection {
            current_revision: stale.current_revision,
        }
    } else {
        ProjectionCommandError::Operation {
            message: format!("{error:#}"),
        }
    }
}

/// Resolve a context descriptor to concrete track ids — under the projection
/// guard, with the same plan and the same SQL the view used — so the queue or
/// a playlist holds exactly the tracks the context names rather than an
/// approximation made in the frontend by expanding albums.
pub(crate) fn resolve_context(
    state: &AppState,
    ctx: &PlayContext,
) -> Result<Vec<i64>, ProjectionCommandError> {
    let expected_revision = match ctx {
        PlayContext::LibraryView {
            projection_revision,
            ..
        }
        | PlayContext::ValueTracks {
            projection_revision,
            ..
        } => Some(*projection_revision),
        _ => None,
    };
    let projection = state
        .plugins
        .projection(expected_revision)
        .map_err(projection_error)?;
    state
        .db
        .resolve_context_with_projection(
            ctx,
            &|mode| projection.facet_plan(mode),
            projection.prefer_romanized(),
        )
        .map_err(|error| ProjectionCommandError::Operation {
            message: error.to_string(),
        })
}

fn operation_error(error: impl std::fmt::Display) -> ProjectionCommandError {
    ProjectionCommandError::Operation {
        message: error.to_string(),
    }
}

fn filter_for(query: Option<&str>) -> Result<Option<SearchFilter>, ProjectionCommandError> {
    match query {
        Some(q) if !q.trim().is_empty() => crescendo_core::search::compile(q)
            .map(Some)
            .map_err(operation_error),
        _ => Ok(None),
    }
}

/// The music folder before one is chosen: the user's XDG music directory.
fn default_library_path() -> String {
    dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Music")))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Releases up to 1.0.2 fell back to the developer's own music folder when no
/// folder was set. A library scanned from there keeps it, saved as a real
/// preference; every other library gets the XDG music folder. Saving it means
/// Settings shows the folder a scan will actually read.
pub fn ensure_library_path(db: &crescendo_core::Db) {
    const PREVIOUS_DEFAULT: &str = "/mnt/Mythic_Meme/Music/";
    if !matches!(db.prefs_get("library_path"), Ok(None)) {
        return;
    }
    let scanned_from_previous_default = db
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM tracks WHERE path LIKE ?1 || '%')",
                [PREVIOUS_DEFAULT],
                |row| row.get::<_, bool>(0),
            )?)
        })
        .unwrap_or(false);
    let path = if scanned_from_previous_default {
        PREVIOUS_DEFAULT.to_string()
    } else {
        default_library_path()
    };
    if let Err(error) = db.prefs_set("library_path", &path) {
        log::warn!("could not save the music folder: {error}");
    }
}

#[tauri::command]
pub async fn scan_start(
    app: AppHandle,
    state: State<'_, AppState>,
    full: bool,
) -> Result<(), String> {
    // The Forte poll holds the lock briefly and often; it must not refuse a scan.
    let lock = state.library_lock.clone();
    let scanning_guard = tauri::async_runtime::spawn_blocking(move || {
        lock.acquire_waiting(LibraryWriter::Scan, Duration::from_secs(10))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|holder| holder.busy_message().to_string())?;
    state.scan_cancel.store(false, Ordering::SeqCst);

    let db = state.db.clone();
    let art = state.art.clone();
    let cancel = state.scan_cancel.clone();
    let root = db
        .prefs_get("library_path")
        .ok()
        .flatten()
        .unwrap_or_else(default_library_path);
    let threads: usize = db
        .prefs_get("scan_threads")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4)
        // The setting's own range. A typed 100000 made thread spawning panic
        // and left the scan latched on.
        .clamp(1, 16);

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
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                scan(
                    &db,
                    &opts,
                    &move |p| {
                        let _ = progress_app.emit("scan:progress", &p);
                    },
                    &cancel,
                )
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the scanner stopped unexpectedly")));
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

/// Whether a scan is running, for a webview that reloaded during one.
#[tauri::command]
pub async fn scan_running(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(is_scanning(&state.library_lock))
}

fn is_scanning(lock: &LibraryLock) -> bool {
    lock.holder() == Some(LibraryWriter::Scan)
}

#[cfg(test)]
mod tests {
    use super::{is_scanning, LibraryLock, LibraryWriter, ProjectionCommandError};
    use std::sync::Arc;

    #[test]
    fn only_a_scan_reads_as_scanning() {
        let lock = Arc::new(LibraryLock::default());
        assert!(!is_scanning(&lock));
        {
            let _editor = lock.try_acquire(LibraryWriter::TagEditor).unwrap();
            assert!(!is_scanning(&lock));
        }
        let _scan = lock.try_acquire(LibraryWriter::Scan).unwrap();
        assert!(is_scanning(&lock));
    }

    #[test]
    fn stale_projection_errors_are_typed_for_frontend_retry() {
        assert_eq!(
            serde_json::to_value(ProjectionCommandError::StaleProjection {
                current_revision: 42,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "stale_projection",
                "current_revision": 42
            })
        );
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
) -> Result<BrowseValuesResult, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state.plugins.projection(None).map_err(projection_error)?;
    let plan = projection.facet_plan(&mode);
    let values = state
        .db
        .browse_values_with_plan(plan.as_ref(), filter.as_ref())
        .map_err(operation_error)?;
    Ok(BrowseValuesResult {
        projection_revision: projection.revision(),
        values,
    })
}

#[tauri::command]
pub async fn browse_albums(
    state: State<'_, AppState>,
    filters: Vec<FacetSelection>,
    query: Option<String>,
    projection_revision: u64,
) -> Result<Vec<Album>, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    state
        .db
        .browse_albums_with_plan(
            &projection.facet_filters(&filters),
            filter.as_ref(),
            projection.prefer_romanized(),
        )
        .map_err(operation_error)
}

#[tauri::command]
pub async fn album_tracks(state: State<'_, AppState>, album_id: i64) -> Result<Vec<Track>, String> {
    state.db.album_tracks(album_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn track_count(
    state: State<'_, AppState>,
    query: Option<String>,
    filters: Vec<FacetSelection>,
    projection_revision: u64,
) -> Result<i64, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    state
        .db
        .track_count_with_plan(filter.as_ref(), &projection.facet_filters(&filters))
        .map_err(operation_error)
}

#[tauri::command]
pub async fn tracks_range(
    state: State<'_, AppState>,
    query: Option<String>,
    filters: Vec<FacetSelection>,
    projection_revision: u64,
    sort: Option<TrackSort>,
    offset: i64,
    limit: i64,
) -> Result<Vec<Track>, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    state
        .db
        .tracks_range_with_plan(
            filter.as_ref(),
            &projection.facet_filters(&filters),
            projection.prefer_romanized(),
            sort,
            offset,
            limit.clamp(1, 2000),
        )
        .map_err(operation_error)
}

#[tauri::command]
pub async fn track_index(
    state: State<'_, AppState>,
    query: Option<String>,
    filters: Vec<FacetSelection>,
    projection_revision: u64,
    sort: Option<TrackSort>,
    track_id: i64,
) -> Result<Option<i64>, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    state
        .db
        .track_index_with_plan(
            filter.as_ref(),
            &projection.facet_filters(&filters),
            projection.prefer_romanized(),
            sort,
            track_id,
        )
        .map_err(operation_error)
}

#[tauri::command]
pub async fn track_groups(
    state: State<'_, AppState>,
    query: Option<String>,
    filters: Vec<FacetSelection>,
    projection_revision: u64,
    sort: Option<TrackSort>,
) -> Result<Vec<TrackGroup>, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    state
        .db
        .track_groups_with_plan(
            filter.as_ref(),
            &projection.facet_filters(&filters),
            projection.prefer_romanized(),
            sort,
        )
        .map_err(operation_error)
}

#[tauri::command]
pub async fn album_by_id(
    state: State<'_, AppState>,
    album_id: i64,
) -> Result<Option<Album>, String> {
    state.db.album_by_id(album_id).map_err(|e| e.to_string())
}

/// Byte ranges to tint in the search bar, one per top-level term.
#[tauri::command]
pub async fn search_highlight(
    query: String,
) -> Result<Vec<crescendo_core::search::QuerySpan>, String> {
    Ok(crescendo_core::search::highlight(&query))
}

/// Set a track's rating in stars (0.5 to 5 in half steps), or clear it with
/// `null`. The player is told, so weighted shuffle follows the change.
#[tauri::command]
pub async fn track_set_rating(
    state: State<'_, AppState>,
    track_id: i64,
    rating: Option<f64>,
) -> Result<(), String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || db.set_rating(track_id, rating))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    if let Err(error) = state.player.send(crate::player::Cmd::RatingsChanged) {
        log::warn!("player not told of a rating change: {error}");
    }
    Ok(())
}
