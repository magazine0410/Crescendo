use crescendo_core::updater::{self, InstallResult, UpdateCheck};
use tokio::sync::Semaphore;

/// Only one elevation/install attempt may be active. The About UI disables its
/// button too, but the command boundary must enforce this independently.
static INSTALL_PERMITS: Semaphore = Semaphore::const_new(1);

#[tauri::command]
pub async fn update_check() -> Result<UpdateCheck, String> {
    let cache_dir = crate::state::cache_dir();
    tauri::async_runtime::spawn_blocking(move || {
        updater::check(env!("CARGO_PKG_VERSION"), &cache_dir)
    })
    .await
    .map_err(|error| format!("update check task failed: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_install(version: String) -> Result<InstallResult, String> {
    let _permit = INSTALL_PERMITS
        .try_acquire()
        .map_err(|_| "An update installation is already running".to_string())?;
    let cache_dir = crate::state::cache_dir();
    tauri::async_runtime::spawn_blocking(move || {
        updater::install(env!("CARGO_PKG_VERSION"), &version, &cache_dir)
    })
    .await
    .map_err(|error| format!("update installation task failed: {error}"))?
    .map_err(|error| error.to_string())
}
