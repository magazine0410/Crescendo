use crate::state::AppState;
use crescendo_core::lastfm::{AlbumInfo, ArtistInfo};
use crescendo_core::lyrics::LyricsResult;
use std::collections::HashMap;
use tauri::{AppHandle, Manager, State, WebviewUrl};
use tokio::sync::Semaphore;

#[derive(serde::Serialize)]
pub struct AboutDependency {
    name: &'static str,
    version: &'static str,
}

#[derive(serde::Serialize)]
pub struct AboutInfo {
    version: &'static str,
    repository_url: &'static str,
    dependencies: Vec<AboutDependency>,
}

/// Bound concurrent waveform decodes (each fully decodes a track). Skipping
/// tracks quickly could otherwise stack several CPU-heavy decodes at once.
static WAVEFORM_PERMITS: Semaphore = Semaphore::const_new(2);

/// Bound concurrent lyrics lookups: each can run up to three VocaDB/LRCLIB
/// requests with 15 s timeouts, so skipping through a track list must not be
/// able to stack an unbounded number of blocking threads on the network.
static LYRICS_PERMITS: Semaphore = Semaphore::const_new(2);

#[tauri::command]
pub async fn prefs_get_all(state: State<'_, AppState>) -> Result<HashMap<String, String>, String> {
    let mut prefs = state.db.prefs_get_all().map_err(|e| e.to_string())?;
    // The visualizer restores this only inside the trusted backend command;
    // the webview has no reason to receive a filesystem path it cannot use.
    prefs.remove("visualizer_import_path");
    Ok(prefs)
}

#[tauri::command]
pub async fn prefs_set(
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    if key == "visualizer_import_path" {
        return Err("that preference is managed by the projectM folder chooser".into());
    }
    state.db.prefs_set(&key, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn about_info() -> AboutInfo {
    AboutInfo {
        version: env!("CARGO_PKG_VERSION"),
        repository_url: "https://github.com/magazine0410/Crescendo",
        dependencies: vec![
            AboutDependency {
                name: "Tauri",
                version: env!("CRESCENDO_DEP_TAURI"),
            },
            AboutDependency {
                name: "Tauri JS API",
                version: env!("CRESCENDO_DEP_TAURI_API"),
            },
            AboutDependency {
                name: "Tauri Dialog",
                version: env!("CRESCENDO_DEP_TAURI_DIALOG"),
            },
            AboutDependency {
                name: "Tauri Opener",
                version: env!("CRESCENDO_DEP_TAURI_OPENER"),
            },
            AboutDependency {
                name: "Svelte",
                version: env!("CRESCENDO_DEP_SVELTE"),
            },
            AboutDependency {
                name: "SvelteKit",
                version: env!("CRESCENDO_DEP_SVELTEKIT"),
            },
            AboutDependency {
                name: "Vite",
                version: env!("CRESCENDO_DEP_VITE"),
            },
            AboutDependency {
                name: "GStreamer bindings",
                version: env!("CRESCENDO_DEP_GSTREAMER"),
            },
            AboutDependency {
                name: "projectM WebAssembly",
                version: "4.1.5",
            },
            AboutDependency {
                name: "Tokio",
                version: env!("CRESCENDO_DEP_TOKIO"),
            },
            AboutDependency {
                name: "rusqlite",
                version: env!("CRESCENDO_DEP_RUSQLITE"),
            },
            AboutDependency {
                name: "Lofty",
                version: env!("CRESCENDO_DEP_LOFTY"),
            },
            AboutDependency {
                name: "image",
                version: env!("CRESCENDO_DEP_IMAGE"),
            },
            AboutDependency {
                name: "ureq",
                version: env!("CRESCENDO_DEP_UREQ"),
            },
            AboutDependency {
                name: "SemVer",
                version: env!("CRESCENDO_DEP_SEMVER"),
            },
            AboutDependency {
                name: "SHA-2",
                version: env!("CRESCENDO_DEP_SHA2"),
            },
            AboutDependency {
                name: "walkdir",
                version: env!("CRESCENDO_DEP_WALKDIR"),
            },
            AboutDependency {
                name: "regex",
                version: env!("CRESCENDO_DEP_REGEX"),
            },
            AboutDependency {
                name: "Serde",
                version: env!("CRESCENDO_DEP_SERDE"),
            },
        ],
    }
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

/// Static lyrics for the Now Playing panel: sidecar `.lrc`/`.txt`, embedded
/// tag, then VocaDB/LRCLIB (cached). Local sources are read live on each call.
#[tauri::command]
pub async fn lyrics_for_track(
    state: State<'_, AppState>,
    track_id: i64,
) -> Result<Option<LyricsResult>, String> {
    let _permit = LYRICS_PERMITS.acquire().await.map_err(|e| e.to_string())?;
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(track) = db.track_by_id(track_id).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        crescendo_core::lyrics::for_track(&db, &track).map_err(|e| e.to_string())
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

/// Path to the freedesktop autostart entry that launches Crescendo on login.
#[cfg(target_os = "linux")]
fn autostart_desktop_path() -> Result<std::path::PathBuf, String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs_home().map(|h| h.join(".config")))
        .ok_or_else(|| "no config directory".to_string())?;
    Ok(base.join("autostart").join("crescendo.desktop"))
}

#[cfg(target_os = "linux")]
fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// Whether Crescendo is set to launch when the user logs in.
#[tauri::command]
pub async fn autostart_get() -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        Ok(autostart_desktop_path()?.exists())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(false)
    }
}

/// First match for `cmd` on `$PATH`, if any.
#[cfg(target_os = "linux")]
fn find_on_path(cmd: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|d| d.join(cmd)).find(|p| p.is_file())
}

/// The command the autostart entry should launch (already shell-quoted).
///
/// A debug build (`npm run tauri dev`) loads the Vite dev-server URL rather than
/// bundled assets, so it can't start on its own at login — it would just show
/// "Could not connect to localhost". Enabling autostart from such a build should
/// still boot the *installed* release app, so point the entry at `crescendo` on
/// PATH instead of the current (dev) executable. A release build loads its
/// bundled assets and works wherever it lives, so use its own path directly.
#[cfg(target_os = "linux")]
fn autostart_exec() -> Result<String, String> {
    if cfg!(debug_assertions) {
        if let Some(installed) = find_on_path("crescendo") {
            return Ok(format!("\"{}\"", installed.display()));
        }
        return Err("Autostart needs the installed Crescendo app, but no `crescendo` \
                    was found on PATH. Install the .deb/.rpm and enable this from the \
                    installed app rather than from `tauri dev`."
            .into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(format!("\"{}\"", exe.display()))
}

/// Enable or disable launching Crescendo on login by writing/removing a
/// freedesktop autostart `.desktop` entry.
#[tauri::command]
pub async fn autostart_set(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let path = autostart_desktop_path()?;
        if enabled {
            let exec = autostart_exec()?;
            let entry = format!(
                "[Desktop Entry]\n\
                 Type=Application\n\
                 Name=Crescendo\n\
                 Exec={exec}\n\
                 Terminal=false\n\
                 X-GNOME-Autostart-enabled=true\n"
            );
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, entry).map_err(|e| e.to_string())?;
        } else if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = enabled;
        Ok(())
    }
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

/// Open or focus the compact About window.
#[tauri::command]
pub async fn open_about_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("about") {
        win.unminimize().map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(&app, "about", WebviewUrl::App("/about".into()))
        .title("About Crescendo")
        .inner_size(500.0, 600.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}
