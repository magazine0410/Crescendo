use crate::player::{Cmd, PlayerState};
use crate::state::{AppState, ProjectMGrants};
use crescendo_core::model::{PlayContext, Track};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use walkdir::WalkDir;

#[derive(Debug, Serialize)]
pub struct ProjectMPresetEntry {
    pub id: String,
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct ProjectMAssetEntry {
    pub id: String,
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct ProjectMImport {
    pub grant: String,
    pub folder_name: String,
    pub presets: Vec<ProjectMPresetEntry>,
    pub assets: Vec<ProjectMAssetEntry>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ProjectMPresetFile {
    pub id: String,
    pub name: String,
    pub data: String,
}

const MAX_PRESETS: usize = 20_000;
const MAX_PRESET_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ASSETS: usize = 2_048;
const MAX_ASSET_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TOTAL_ASSET_BYTES: u64 = 16 * 1024 * 1024;

#[tauri::command]
pub async fn player_play_context(
    state: State<'_, AppState>,
    ctx: PlayContext,
    start_index: usize,
) -> Result<(), String> {
    state.player.send(Cmd::PlayContext { ctx, start_index })
}

#[tauri::command]
pub async fn player_jump_to(state: State<'_, AppState>, index: usize) -> Result<(), String> {
    state.player.send(Cmd::JumpTo(index))
}

#[tauri::command]
pub async fn player_toggle_pause(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::TogglePause)
}

#[tauri::command]
pub async fn player_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Stop)
}

#[tauri::command]
pub async fn player_next(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Next)
}

#[tauri::command]
pub async fn player_prev(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::Prev)
}

#[tauri::command]
pub async fn player_seek(state: State<'_, AppState>, ms: u64) -> Result<(), String> {
    state.player.send(Cmd::Seek(ms))
}

#[tauri::command]
pub async fn player_set_volume(state: State<'_, AppState>, volume: f64) -> Result<(), String> {
    state.player.send(Cmd::SetVolume(volume))?;
    let _ = state.db.prefs_set("volume", &format!("{volume:.3}"));
    Ok(())
}

#[tauri::command]
pub async fn player_set_muted(state: State<'_, AppState>, muted: bool) -> Result<(), String> {
    state.player.send(Cmd::SetMuted(muted))
}

#[tauri::command]
pub async fn player_set_shuffle(state: State<'_, AppState>, shuffle: bool) -> Result<(), String> {
    state.player.send(Cmd::SetShuffle(shuffle))
}

#[tauri::command]
pub async fn player_cycle_repeat(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::CycleRepeat)
}

#[tauri::command]
pub async fn player_set_stop_after_current(
    state: State<'_, AppState>,
    on: bool,
) -> Result<(), String> {
    state.player.send(Cmd::SetStopAfterCurrent(on))
}

#[tauri::command]
pub async fn player_set_spectrum_enabled(
    state: State<'_, AppState>,
    on: bool,
) -> Result<(), String> {
    state.player.send(Cmd::SetSpectrumEnabled(on))
}

#[tauri::command]
pub async fn player_set_projectm_enabled(
    state: State<'_, AppState>,
    on: bool,
) -> Result<(), String> {
    state.player.send(Cmd::SetProjectMEnabled(on))
}

/// Return all PCM accumulated since the previous pull as raw bytes. The
/// player-side queue is tiny and drops its oldest chunks under pressure, so a
/// delayed webview can neither grow memory nor add visual latency.
#[tauri::command]
pub async fn player_take_projectm_pcm(
    state: State<'_, AppState>,
) -> Result<tauri::ipc::Response, String> {
    Ok(tauri::ipc::Response::new(state.player.take_projectm_pcm()?))
}

/// Open the trusted native folder chooser, index the selected MilkDrop pack,
/// and replace the session's approved import root. The webview receives only
/// an opaque grant, never authority to choose an arbitrary backend path.
#[tauri::command]
pub async fn projectm_choose_folder(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ProjectMImport>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Import projectM preset folder")
        .pick_folder(move |path| {
            let _ = sender.send(path);
        });
    let Some(selected) = receiver.await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|e| e.to_string())?;
    let db = state.db.clone();
    let grants = state.projectm_grants.clone();
    let imported = tauri::async_runtime::spawn_blocking(move || {
        approve_projectm_folder(&db, &grants, path, true)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(Some(imported))
}

/// Restore only the path the backend previously persisted after a trusted
/// chooser selection. `prefs_set` cannot modify this sensitive preference.
#[tauri::command]
pub async fn projectm_restore_folder(
    state: State<'_, AppState>,
) -> Result<Option<ProjectMImport>, String> {
    let Some(path) = state
        .db
        .prefs_get("visualizer_import_path")
        .map_err(|e| e.to_string())?
        .filter(|path| !path.is_empty())
    else {
        return Ok(None);
    };
    let db = state.db.clone();
    let grants = state.projectm_grants.clone();
    let imported = tauri::async_runtime::spawn_blocking(move || {
        approve_projectm_folder(&db, &grants, PathBuf::from(path), false)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(Some(imported))
}

fn approve_projectm_folder(
    db: &crescendo_core::db::Db,
    grants: &std::sync::Mutex<ProjectMGrants>,
    path: PathBuf,
    persist: bool,
) -> Result<ProjectMImport, String> {
    let root = path
        .canonicalize()
        .map_err(|_| "The selected projectM preset folder no longer exists".to_string())?;
    if !root.is_dir() {
        return Err("The selected projectM preset folder no longer exists".into());
    }
    let mut imported = index_projectm_folder(&root)?;
    if persist {
        let path = root
            .to_str()
            .ok_or_else(|| "The selected projectM folder path is not valid UTF-8".to_string())?;
        db.prefs_set("visualizer_import_path", path)
            .map_err(|e| e.to_string())?;
    }
    // Do not revoke the currently working grant until indexing and any
    // persistence have both succeeded.
    let grant = grants
        .lock()
        .map_err(|_| "The projectM import grant store is unavailable".to_string())?
        .register(root);
    imported.grant = grant;
    Ok(imported)
}

fn index_projectm_folder(root: &Path) -> Result<ProjectMImport, String> {
    if !root.is_dir() {
        return Err("The selected projectM preset folder no longer exists".into());
    }

    let mut presets = Vec::new();
    let mut assets = Vec::new();
    let mut warnings = Vec::new();
    let mut asset_bytes = 0u64;
    let mut asset_names = HashSet::new();
    let mut skipped_large_presets = 0usize;
    let mut skipped_extra_presets = 0usize;
    let mut skipped_assets = 0usize;
    let mut skipped_duplicate_assets = 0usize;
    let mut unreadable_entries = 0usize;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                unreadable_entries += 1;
                continue;
            }
        };
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
            let size = match entry.metadata() {
                Ok(metadata) => metadata.len(),
                Err(_) => {
                    unreadable_entries += 1;
                    continue;
                }
            };
            if size > MAX_PRESET_BYTES {
                skipped_large_presets += 1;
                continue;
            }
            if presets.len() >= MAX_PRESETS {
                skipped_extra_presets += 1;
                continue;
            }
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
            presets.push(ProjectMPresetEntry {
                id: relative,
                name,
                bytes: size,
            });
        } else if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "bmp" | "tga" | "dds") {
            let size = match entry.metadata() {
                Ok(metadata) => metadata.len(),
                Err(_) => {
                    unreadable_entries += 1;
                    continue;
                }
            };
            if assets.len() >= MAX_ASSETS
                || size > MAX_ASSET_BYTES
                || asset_bytes.saturating_add(size) > MAX_TOTAL_ASSET_BYTES
            {
                skipped_assets += 1;
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // projectM texture references are basename-based, and the WASM
            // filesystem flattens imports into one directory. Keep the first
            // duplicate instead of silently overwriting it later.
            if !asset_names.insert(name.to_lowercase()) {
                skipped_duplicate_assets += 1;
                continue;
            }
            let relative = file
                .strip_prefix(root)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned();
            asset_bytes += size;
            assets.push(ProjectMAssetEntry {
                id: relative,
                name,
                bytes: size,
            });
        }
    }

    presets.sort_by_cached_key(|preset| preset.name.to_lowercase());
    if presets.is_empty() {
        return Err("No .milk presets were found in that folder".into());
    }
    if skipped_large_presets > 0 {
        warnings.push(format!(
            "Skipped {skipped_large_presets} preset(s) larger than 2 MiB"
        ));
    }
    if skipped_extra_presets > 0 {
        warnings.push(format!(
            "Loaded the first {MAX_PRESETS} presets and skipped {skipped_extra_presets} more"
        ));
    }
    if skipped_assets > 0 {
        warnings.push(format!(
            "Skipped {skipped_assets} texture(s) beyond the 8 MiB per-file, 16 MiB total, or {MAX_ASSETS}-file limits"
        ));
    }
    if skipped_duplicate_assets > 0 {
        warnings.push(format!(
            "Skipped {skipped_duplicate_assets} texture(s) whose filename duplicated an earlier texture"
        ));
    }
    if unreadable_entries > 0 {
        warnings.push(format!(
            "Skipped {unreadable_entries} file(s) or folder(s) that could not be read"
        ));
    }
    Ok(ProjectMImport {
        grant: String::new(),
        folder_name: root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Imported presets")
            .to_owned(),
        presets,
        assets,
        warnings,
    })
}

#[tauri::command]
pub async fn projectm_load_preset(
    state: State<'_, AppState>,
    grant: String,
    id: String,
) -> Result<ProjectMPresetFile, String> {
    let root = resolve_projectm_grant(&state.projectm_grants, &grant)?;
    tauri::async_runtime::spawn_blocking(move || load_projectm_preset(&root, &id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn projectm_load_asset(
    state: State<'_, AppState>,
    grant: String,
    id: String,
) -> Result<tauri::ipc::Response, String> {
    let root = resolve_projectm_grant(&state.projectm_grants, &grant)?;
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        let file = resolve_import_file(&root, &id, &["jpg", "jpeg", "png", "bmp", "tga", "dds"])?;
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        if size > MAX_ASSET_BYTES {
            return Err("The selected projectM texture is larger than 8 MiB".into());
        }
        std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

fn resolve_projectm_grant(
    grants: &std::sync::Mutex<ProjectMGrants>,
    grant: &str,
) -> Result<PathBuf, String> {
    grants
        .lock()
        .map_err(|_| "The projectM import grant store is unavailable".to_string())?
        .resolve(grant)
        .ok_or_else(|| "The projectM import grant has expired".to_string())
}

fn load_projectm_preset(root: &Path, id: &str) -> Result<ProjectMPresetFile, String> {
    let file = resolve_import_file(root, id, &["milk"])?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > MAX_PRESET_BYTES {
        return Err("The selected projectM preset is larger than 2 MiB".into());
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(ProjectMPresetFile {
        id: id.to_owned(),
        name: file
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Unnamed preset")
            .to_owned(),
        data: decode_milk_text(&bytes),
    })
}

fn resolve_import_file(root: &Path, id: &str, extensions: &[&str]) -> Result<PathBuf, String> {
    let relative = Path::new(id);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Invalid projectM import path".into());
    }
    let file = root
        .join(relative)
        .canonicalize()
        .map_err(|_| "The selected projectM import file no longer exists".to_string())?;
    if !file.starts_with(&root) || !file.is_file() {
        return Err("Invalid projectM import path".into());
    }
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !extensions.contains(&extension.as_str()) {
        return Err("Unsupported projectM import file".into());
    }
    Ok(file)
}

/// MilkDrop packs commonly contain Windows-1252 text. Preserve valid UTF-8,
/// otherwise decode the single-byte Windows code page instead of inserting
/// replacement characters through `from_utf8_lossy`.
fn decode_milk_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    bytes
        .iter()
        .map(|byte| match byte {
            0x80 => '\u{20ac}',
            0x82 => '\u{201a}',
            0x83 => '\u{0192}',
            0x84 => '\u{201e}',
            0x85 => '\u{2026}',
            0x86 => '\u{2020}',
            0x87 => '\u{2021}',
            0x88 => '\u{02c6}',
            0x89 => '\u{2030}',
            0x8a => '\u{0160}',
            0x8b => '\u{2039}',
            0x8c => '\u{0152}',
            0x8e => '\u{017d}',
            0x91 => '\u{2018}',
            0x92 => '\u{2019}',
            0x93 => '\u{201c}',
            0x94 => '\u{201d}',
            0x95 => '\u{2022}',
            0x96 => '\u{2013}',
            0x97 => '\u{2014}',
            0x98 => '\u{02dc}',
            0x99 => '\u{2122}',
            0x9a => '\u{0161}',
            0x9b => '\u{203a}',
            0x9c => '\u{0153}',
            0x9e => '\u{017e}',
            0x9f => '\u{0178}',
            0x81 | 0x8d | 0x8f | 0x90 | 0x9d => '\u{fffd}',
            byte => char::from(*byte),
        })
        .collect()
}

#[cfg(test)]
mod projectm_import_tests {
    use super::{
        decode_milk_text, index_projectm_folder, load_projectm_preset, resolve_projectm_grant,
    };
    use crate::state::ProjectMGrants;
    use std::path::Path;
    use std::sync::Mutex;
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

        let imported = index_projectm_folder(&root).unwrap();
        assert_eq!(imported.presets.len(), 2);
        assert_eq!(imported.presets[0].name, "alpha");
        assert_eq!(imported.presets[1].name, "Zulu");
        assert!(imported.presets[0].id.ends_with("nested/alpha.MILK"));
        assert_eq!(imported.assets.len(), 1);
        assert_eq!(imported.assets[0].name, "cloud.png");
        assert_eq!(imported.assets[0].bytes, 4);
        assert!(imported.warnings.is_empty());

        let preset = load_projectm_preset(&root, &imported.presets[0].id).unwrap();
        assert_eq!(preset.name, "alpha");
        assert!(preset.data.contains("fRating=5"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn decodes_windows_1252_preset_text() {
        assert_eq!(
            decode_milk_text(b"name=Martin\x92s \x93preset\x94"),
            "name=Martin’s “preset”"
        );
    }

    #[test]
    fn bounds_large_import_files_and_rejects_traversal() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "crescendo-projectm-bounds-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("valid.milk"), "[preset00]").unwrap();
        std::fs::File::create(root.join("huge.milk"))
            .unwrap()
            .set_len(2 * 1024 * 1024 + 1)
            .unwrap();
        std::fs::File::create(root.join("huge.png"))
            .unwrap()
            .set_len(8 * 1024 * 1024 + 1)
            .unwrap();

        let imported = index_projectm_folder(&root).unwrap();
        assert_eq!(imported.presets.len(), 1);
        assert!(imported
            .warnings
            .iter()
            .any(|warning| warning.contains("preset")));
        assert!(imported
            .warnings
            .iter()
            .any(|warning| warning.contains("texture")));
        assert!(load_projectm_preset(&root, "../valid.milk").is_err());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacing_a_projectm_root_revokes_its_old_grant() {
        let grants = Mutex::new(ProjectMGrants::default());
        let first = grants.lock().unwrap().register("/first".into());
        assert_eq!(resolve_projectm_grant(&grants, &first).unwrap(), Path::new("/first"));
        let second = grants.lock().unwrap().register("/second".into());
        assert!(resolve_projectm_grant(&grants, &first).is_err());
        assert_eq!(resolve_projectm_grant(&grants, &second).unwrap(), Path::new("/second"));
    }
}

#[tauri::command]
pub async fn player_get_state(state: State<'_, AppState>) -> Result<PlayerState, String> {
    state.player.state()
}

#[tauri::command]
pub async fn queue_add(state: State<'_, AppState>, track_ids: Vec<i64>) -> Result<(), String> {
    state.player.send(Cmd::QueueAdd(track_ids))
}

#[tauri::command]
pub async fn queue_remove(state: State<'_, AppState>, positions: Vec<usize>) -> Result<(), String> {
    state.player.send(Cmd::QueueRemove(positions))
}

#[tauri::command]
pub async fn queue_clear(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(Cmd::QueueClear)
}

#[tauri::command]
pub async fn queue_list(state: State<'_, AppState>) -> Result<Vec<Track>, String> {
    let ids = state.player.queue_ids()?;
    state.db.tracks_by_ids(&ids).map_err(|e| e.to_string())
}
