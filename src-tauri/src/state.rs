use crate::player::PlayerHandle;
use crate::waveform::WaveformStore;
use crescendo_core::art::ArtStore;
use crescendo_core::db::Db;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct AppState {
    pub db: Arc<Db>,
    pub art: Arc<ArtStore>,
    pub waveform: Arc<WaveformStore>,
    pub player: PlayerHandle,
    pub scanning: Arc<AtomicBool>,
    pub scan_cancel: Arc<AtomicBool>,
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("crescendo")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join("crescendo")
}

pub fn db_path() -> PathBuf {
    data_dir().join("library.db")
}
