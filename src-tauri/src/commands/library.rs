use crate::state::AppState;
use crescendo_core::model::{
    Album, BrowseMode, BrowseValue, LibraryStats, PlayContext, Track, TrackGroup, TrackSort,
};
use crescendo_core::plugins::StaleProjection;
use crescendo_core::scanner::{scan, ScanOptions};
use crescendo_core::search::SearchFilter;
use serde::Serialize;
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
    let plan = match ctx {
        PlayContext::ValueTracks { mode, .. } => projection.facet_plan(mode),
        _ => None,
    };
    state
        .db
        .resolve_context_with_projection(ctx, plan.as_ref(), projection.prefer_romanized())
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
    use super::{ProjectionCommandError, ScanningGuard};
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
    mode: BrowseMode,
    values: Option<Vec<String>>,
    query: Option<String>,
    projection_revision: u64,
) -> Result<Vec<Album>, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    let plan = projection.facet_plan(&mode);
    state
        .db
        .browse_albums_with_plan(
            plan.as_ref(),
            &values.unwrap_or_default(),
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
    mode: BrowseMode,
    values: Option<Vec<String>>,
    projection_revision: u64,
) -> Result<i64, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    let plan = projection.facet_plan(&mode);
    state
        .db
        .track_count_with_plan(filter.as_ref(), plan.as_ref(), &values.unwrap_or_default())
        .map_err(operation_error)
}

#[tauri::command]
pub async fn tracks_range(
    state: State<'_, AppState>,
    query: Option<String>,
    mode: BrowseMode,
    values: Option<Vec<String>>,
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
    let plan = projection.facet_plan(&mode);
    state
        .db
        .tracks_range_with_plan(
            filter.as_ref(),
            plan.as_ref(),
            &values.unwrap_or_default(),
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
    mode: BrowseMode,
    values: Option<Vec<String>>,
    projection_revision: u64,
    sort: Option<TrackSort>,
    track_id: i64,
) -> Result<Option<i64>, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    let plan = projection.facet_plan(&mode);
    state
        .db
        .track_index_with_plan(
            filter.as_ref(),
            plan.as_ref(),
            &values.unwrap_or_default(),
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
    mode: BrowseMode,
    values: Option<Vec<String>>,
    projection_revision: u64,
    sort: Option<TrackSort>,
) -> Result<Vec<TrackGroup>, ProjectionCommandError> {
    let filter = filter_for(query.as_deref())?;
    let projection = state
        .plugins
        .projection(Some(projection_revision))
        .map_err(projection_error)?;
    let plan = projection.facet_plan(&mode);
    state
        .db
        .track_groups_with_plan(
            filter.as_ref(),
            plan.as_ref(),
            &values.unwrap_or_default(),
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
