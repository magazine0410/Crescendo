use crate::player::PlayerHandle;
use crate::waveform::WaveformStore;
use crescendo_core::art::ArtStore;
use crescendo_core::db::Db;
use crescendo_core::plugins::PluginManager;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct ProjectMGrants {
    generation: u64,
    current: Option<(String, PathBuf)>,
}

impl ProjectMGrants {
    /// Replace the session's approved import root and invalidate the previous
    /// grant. The token is opaque rather than a filesystem path; secrecy is not
    /// required because only a native chooser or the backend preference restore
    /// can create one.
    pub fn register(&mut self, root: PathBuf) -> String {
        self.generation = self.generation.wrapping_add(1);
        let token = format!("projectm-{:016x}", self.generation);
        self.current = Some((token.clone(), root));
        token
    }

    pub fn resolve(&self, token: &str) -> Option<PathBuf> {
        self.current
            .as_ref()
            .filter(|(current, _)| current == token)
            .map(|(_, root)| root.clone())
    }
}

pub struct AppState {
    pub db: Arc<Db>,
    pub art: Arc<ArtStore>,
    pub waveform: Arc<WaveformStore>,
    pub plugins: Arc<PluginManager>,
    pub player: PlayerHandle,
    pub scrobbler: Arc<crescendo_core::lastfm::scrobble::Scrobbler>,
    pub projectm_grants: Arc<Mutex<ProjectMGrants>>,
    pub scanning: Arc<AtomicBool>,
    pub scan_cancel: Arc<AtomicBool>,
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crescendo")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crescendo")
}

pub fn db_path() -> PathBuf {
    data_dir().join("library.db")
}
