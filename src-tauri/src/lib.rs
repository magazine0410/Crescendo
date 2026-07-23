mod art_protocol;
mod commands;
mod player;
mod state;
pub mod waveform;

use crescendo_core::art::ArtStore;
use crescendo_core::db::Db;
use state::AppState;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,lofty=error"),
    )
    .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("crescendo-art", art_protocol::handle)
        .setup(|app| {
            let db = Arc::new(Db::open(&state::db_path())?);
            if let Ok(stats) = db.library_stats() {
                log::info!("library opened: {} tracks, {} albums", stats.tracks, stats.albums);
            }
            let art = Arc::new(ArtStore::new(&state::cache_dir())?);
            let waveform = Arc::new(waveform::WaveformStore::new(&state::cache_dir())?);
            let player = player::spawn(app.handle().clone(), db.clone())?;

            // Restore persisted volume before the first play.
            if let Ok(Some(v)) = db.prefs_get("volume") {
                if let Ok(v) = v.parse::<f64>() {
                    player.send(player::Cmd::SetVolume(v));
                }
            }

            app.manage(AppState {
                db,
                art,
                waveform,
                player,
                scanning: Arc::new(AtomicBool::new(false)),
                scan_cancel: Arc::new(AtomicBool::new(false)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::library::scan_start,
            commands::library::scan_cancel,
            commands::library::library_stats,
            commands::library::browse_values,
            commands::library::browse_albums,
            commands::library::album_tracks,
            commands::library::track_count,
            commands::library::tracks_range,
            commands::library::track_groups,
            commands::library::track_index,
            commands::library::album_by_id,
            commands::library::search_classify,
            commands::player::player_play_context,
            commands::player::player_jump_to,
            commands::player::player_toggle_pause,
            commands::player::player_stop,
            commands::player::player_next,
            commands::player::player_prev,
            commands::player::player_seek,
            commands::player::player_set_volume,
            commands::player::player_set_muted,
            commands::player::player_set_shuffle,
            commands::player::player_cycle_repeat,
            commands::player::player_set_stop_after_current,
            commands::player::player_set_spectrum_enabled,
            commands::player::player_get_state,
            commands::player::queue_add,
            commands::player::queue_remove,
            commands::player::queue_clear,
            commands::player::queue_list,
            commands::playlists::playlist_list,
            commands::playlists::playlist_create,
            commands::playlists::playlist_rename,
            commands::playlists::playlist_delete,
            commands::playlists::playlist_tracks,
            commands::playlists::track_playlist_ids,
            commands::playlists::playlist_add_tracks,
            commands::playlists::playlist_remove_tracks,
            commands::playlists::playlist_reorder,
            commands::playlists::playlist_import_m3u,
            commands::misc::prefs_get_all,
            commands::misc::prefs_set,
            commands::misc::waveform_get,
            commands::misc::lastfm_artist_info,
            commands::misc::lastfm_album_info,
            commands::misc::lastfm_test_key,
            commands::misc::autostart_get,
            commands::misc::autostart_set,
            commands::misc::open_cover_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
