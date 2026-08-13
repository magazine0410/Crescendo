use crate::player::{Cmd, PlayerState};
use crate::state::AppState;
use base64::Engine as _;
use crescendo_core::model::{PlayContext, Track};
use serde::Serialize;
use std::path::Path;
use tauri::State;
use walkdir::WalkDir;

#[derive(Debug, Serialize)]
pub struct ProjectMPresetFile {
    pub id: String,
    pub name: String,
    pub data: String,
}

#[derive(Debug, Serialize)]
pub struct ProjectMAssetFile {
    pub name: String,
    pub data_base64: String,
}

#[derive(Debug, Serialize)]
pub struct ProjectMImport {
    pub folder_name: String,
    pub presets: Vec<ProjectMPresetFile>,
    pub assets: Vec<ProjectMAssetFile>,
}

#[tauri::command]
pub async fn player_play_context(
    state: State<'_, AppState>,
    ctx: PlayContext,
    start_index: usize,
) -> Result<(), String> {
    state.player.send(Cmd::PlayContext { ctx, start_index });
    Ok(())
}

#[tauri::command]
pub async fn player_jump_to(state: State<'_, AppState>, index: usize) -> Result<(), String> {
    state.player.send(Cmd::JumpTo(index));
    Ok(())
}

#[tauri::command]
pub async fn player_toggle_pause(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::TogglePause);
    Ok(())
}

#[tauri::command]
pub async fn player_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Stop);
    Ok(())
}

#[tauri::command]
pub async fn player_next(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Next);
    Ok(())
}

#[tauri::command]
pub async fn player_prev(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Prev);
    Ok(())
}

#[tauri::command]
pub async fn player_seek(state: State<'_, AppState>, ms: u64) -> Result<(), String> {
    state.player.send(Cmd::Seek(ms));
    Ok(())
}

#[tauri::command]
pub async fn player_set_volume(state: State<'_, AppState>, volume: f64) -> Result<(), String> {
    state.player.send(Cmd::SetVolume(volume));
    let _ = state.db.prefs_set("volume", &format!("{volume:.3}"));
    Ok(())
}

#[tauri::command]
pub async fn player_set_muted(state: State<'_, AppState>, muted: bool) -> Result<(), String> {
    state.player.send(Cmd::SetMuted(muted));
    Ok(())
}

#[tauri::command]
pub async fn player_set_shuffle(state: State<'_, AppState>, shuffle: bool) -> Result<(), String> {
    state.player.send(Cmd::SetShuffle(shuffle));
    Ok(())
}

#[tauri::command]
pub async fn player_cycle_repeat(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::CycleRepeat);
    Ok(())
}

#[tauri::command]
pub async fn player_set_stop_after_current(
    state: State<'_, AppState>,
    on: bool,
) -> Result<(), String> {
    state.player.send(Cmd::SetStopAfterCurrent(on));
    Ok(())
}

#[tauri::command]
pub async fn player_set_spectrum_enabled(
    state: State<'_, AppState>,
    on: bool,
) -> Result<(), String> {
    state.player.send(Cmd::SetSpectrumEnabled(on));
    Ok(())
}

#[tauri::command]
pub async fn player_set_projectm_enabled(
    state: State<'_, AppState>,
    on: bool,
) -> Result<(), String> {
    state.player.send(Cmd::SetProjectMEnabled(on));
    Ok(())
}

/// Read a user-selected MilkDrop folder. Presets are text; supported image
/// assets are returned alongside them so the WebAssembly filesystem can make
/// texture-dependent presets work too. Nothing is copied into the library.
#[tauri::command]
pub async fn projectm_import_folder(path: String) -> Result<ProjectMImport, String> {
    tauri::async_runtime::spawn_blocking(move || import_projectm_folder(&path))
        .await
        .map_err(|e| e.to_string())?
}

fn import_projectm_folder(path: &str) -> Result<ProjectMImport, String> {
    const MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;
    const MAX_TOTAL_ASSET_BYTES: u64 = 64 * 1024 * 1024;

    let root = Path::new(path);
    if !root.is_dir() {
        return Err("The selected projectM preset folder no longer exists".into());
    }

    let mut presets = Vec::new();
    let mut assets = Vec::new();
    let mut asset_bytes = 0u64;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let file = entry.path();
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "milk" {
            let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let relative = file
                .strip_prefix(root)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned();
            let name = file
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("Unnamed preset")
                .to_owned();
            presets.push(ProjectMPresetFile {
                id: relative,
                name,
                data: String::from_utf8_lossy(&bytes).into_owned(),
            });
        } else if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "bmp" | "tga" | "dds") {
            let size = entry.metadata().map_err(|e| e.to_string())?.len();
            if size > MAX_ASSET_BYTES || asset_bytes.saturating_add(size) > MAX_TOTAL_ASSET_BYTES {
                continue;
            }
            asset_bytes += size;
            let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            assets.push(ProjectMAssetFile {
                // MilkDrop texture references are basename-based. Flattening
                // also avoids exposing the user's absolute path in the webview.
                name: entry.file_name().to_string_lossy().into_owned(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
        }
    }

    presets.sort_by_cached_key(|preset| preset.name.to_lowercase());
    if presets.is_empty() {
        return Err("No .milk presets were found in that folder".into());
    }
    Ok(ProjectMImport {
        folder_name: root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Imported presets")
            .to_owned(),
        presets,
        assets,
    })
}

#[cfg(test)]
mod projectm_import_tests {
    use super::import_projectm_folder;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn recursively_imports_presets_and_supported_textures() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "crescendo-projectm-import-{}-{nonce}",
            std::process::id()
        ));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("Zulu.milk"), "[preset00]\nfRating=3").unwrap();
        std::fs::write(nested.join("alpha.MILK"), "[preset00]\nfRating=5").unwrap();
        std::fs::write(nested.join("cloud.png"), [0x89, b'P', b'N', b'G']).unwrap();
        std::fs::write(root.join("ignored.txt"), "not a preset").unwrap();

        let imported = import_projectm_folder(root.to_str().unwrap()).unwrap();
        assert_eq!(imported.presets.len(), 2);
        assert_eq!(imported.presets[0].name, "alpha");
        assert_eq!(imported.presets[1].name, "Zulu");
        assert!(imported.presets[0].id.ends_with("nested/alpha.MILK"));
        assert_eq!(imported.assets.len(), 1);
        assert_eq!(imported.assets[0].name, "cloud.png");
        assert_eq!(imported.assets[0].data_base64, "iVBORw==");

        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tauri::command]
pub async fn player_get_state(state: State<'_, AppState>) -> Result<PlayerState, String> {
    Ok(state.player.state())
}

#[tauri::command]
pub async fn queue_add(state: State<'_, AppState>, track_ids: Vec<i64>) -> Result<(), String> {
    state.player.send(Cmd::QueueAdd(track_ids));
    Ok(())
}

#[tauri::command]
pub async fn queue_remove(state: State<'_, AppState>, positions: Vec<usize>) -> Result<(), String> {
    state.player.send(Cmd::QueueRemove(positions));
    Ok(())
}

#[tauri::command]
pub async fn queue_clear(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::QueueClear);
    Ok(())
}

#[tauri::command]
pub async fn queue_list(state: State<'_, AppState>) -> Result<Vec<Track>, String> {
    let ids = state.player.queue_ids();
    state.db.tracks_by_ids(&ids).map_err(|e| e.to_string())
}
