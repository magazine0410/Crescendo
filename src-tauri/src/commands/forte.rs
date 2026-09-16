use crate::state::AppState;
use crescendo_core::forte::{refresh_files, refresh_pending, FileChange};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

struct EditGuard(Arc<AtomicBool>);
impl Drop for EditGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[derive(Deserialize)]
struct SavedFiles {
    forte: u8,
    changes: Vec<FileChange>,
    receipt: Option<String>,
}

/// Pick up independent Forte saves while idle; the same latch prevents a
/// scanner, companion window, and inbox refresh from changing paths at once.
pub fn start_listener(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last_error = String::new();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let state = app.state::<AppState>();
            if state
                .scanning
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                continue;
            }
            let _guard = EditGuard(state.scanning.clone());
            match refresh_pending(&state.db, &crate::state::data_dir().join("forte-inbox")) {
                Ok(changed) => {
                    last_error.clear();
                    if changed {
                        let _ = state.art.invalidate_stale(&state.db);
                        let _ = state.player.send(crate::player::Cmd::RefreshMetadata);
                        let _ = app.emit("library:changed", ());
                        let _ = app.emit("playlist:changed", 0_i64);
                        let _ = app.emit("queue:changed", ());
                    }
                }
                Err(error) => {
                    let message = format!(
                        "Forte saved files, but library refresh needs attention: {error:#}"
                    );
                    if message != last_error {
                        let _ = app.emit("forte:error", &message);
                        last_error = message;
                    }
                }
            }
        }
    });
}

fn executables() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("FORTE_EXECUTABLE") {
        paths.push(path.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            paths.push(parent.join("forte"));
        }
    }
    paths.push(PathBuf::from("forte"));
    if cfg!(debug_assertions) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Forte/target");
        paths.push(root.join("debug/forte"));
        paths.push(root.join("release/forte"));
    }
    paths
}

#[tauri::command]
pub async fn tag_edit_open(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Tag editing is available from the main window".into());
    }
    if ids.is_empty() {
        return Err("Select a song to edit".into());
    }
    // The scanner cannot race the path-preserving refresh after a rename.
    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("Finish the current library scan or tag edit first".into());
    }
    let guard = EditGuard(state.scanning.clone());
    let tracks = state.db.tracks_by_ids(&ids).map_err(|e| e.to_string())?;
    let paths: Vec<_> = tracks
        .iter()
        .filter(|track| !track.missing)
        .map(|track| track.path.clone())
        .collect();
    if paths.is_empty() {
        return Err("The selected files are missing".into());
    }
    let db = state.db.clone();
    let art = state.art.clone();
    let player = state.player.clone();
    let handoff =
        serde_json::json!({"version": 1, "paths": paths, "library_db": crate::state::db_path()});
    tauri::async_runtime::spawn_blocking(move || {
        let mut child = None;
        for path in executables() {
            match Command::new(path).env("FORTE_GUI_HANDOFF", "1")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn() {
                Ok(process) => { child = Some(process); break; }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("Could not open Forte: {error}")),
            }
        }
        let mut child = child.ok_or("Forte is not installed. Install its desktop package, or build the sibling Forte project, then try Tag edit again.")?;
        let written = (|| {
            let mut input = child.stdin.take().ok_or("Forte input unavailable")?;
            serde_json::to_writer(&mut input, &handoff).map_err(|e| e.to_string())?;
            input.flush().map_err(|e| e.to_string())
        })();
        if let Err(error) = written { let _ = child.kill(); let _ = child.wait(); return Err(error); }
        let output = child.stdout.take().ok_or("Forte output unavailable")?;
        std::thread::Builder::new().name("forte-editor".into()).spawn(move || {
            let _guard = guard;
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break; };
                let Ok(saved) = serde_json::from_str::<SavedFiles>(&line) else { continue; };
                if saved.forte != 1 { continue; }
                let refreshed = if saved.receipt.is_some() {
                    refresh_pending(&db, &crate::state::data_dir().join("forte-inbox")).map(|_| ())
                } else { refresh_files(&db, &saved.changes) };
                if let Err(error) = refreshed {
                    let _ = app.emit("forte:error", format!("Files were saved, but Crescendo could not refresh them: {error:#}"));
                    continue;
                }
                let _ = art.invalidate_stale(&db);
                let _ = player.send(crate::player::Cmd::RefreshMetadata);
                let _ = app.emit("library:changed", ());
                let _ = app.emit("playlist:changed", 0_i64);
                let _ = app.emit("queue:changed", ());
            }
            if let Ok(status) = child.wait() {
                if !status.success() { let _ = app.emit("forte:error", "Forte closed unexpectedly. Open Forte to inspect its recovery information."); }
            }
        }).map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|e| e.to_string())?
}
