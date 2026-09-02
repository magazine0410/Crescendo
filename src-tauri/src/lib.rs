mod art_protocol;
mod commands;
mod player;
pub mod plugin_bridge;
mod state;
pub mod waveform;

use crescendo_core::art::ArtStore;
use crescendo_core::db::Db;
use crescendo_core::plugins::wasm::HostEvent;
use crescendo_core::plugins::PluginManager;
use state::AppState;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::Listener;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,lofty=error"))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("crescendo-art", art_protocol::handle)
        .setup(|app| {
            let db = Arc::new(Db::open(&state::db_path())?);
            if let Ok(stats) = db.library_stats() {
                log::info!(
                    "library opened: {} tracks, {} albums",
                    stats.tracks,
                    stats.albums
                );
            }
            let art = Arc::new(ArtStore::new(&state::cache_dir())?);
            let waveform = Arc::new(waveform::WaveformStore::new(&state::cache_dir())?);
            let plugin_root = state::data_dir().join("plugins");
            let plugins = Arc::new(
                PluginManager::start(plugin_root.clone(), db.clone(), env!("CARGO_PKG_VERSION"))
                    .unwrap_or_else(|error| {
                        log::error!("plugin system startup failed: {error:#}");
                        PluginManager::disabled(
                            plugin_root,
                            db.clone(),
                            env!("CARGO_PKG_VERSION"),
                            format!("{error:#}"),
                        )
                    }),
            );
            let player = player::spawn(app.handle().clone(), db.clone())?;
            // The manager starts before the player thread exists, so the
            // bridge is installed here; player.* calls before this point
            // answer Unavailable rather than blocking.
            plugins.install_host_notifier(Arc::new(plugin_bridge::ShellHostNotifier::new(
                app.handle().clone(),
            )));
            let position_mirror = Arc::new(plugin_bridge::PositionMirror::default());
            plugins.install_player_bridge(Arc::new(plugin_bridge::ShellPlayerBridge::new(
                player.clone(),
                position_mirror.clone(),
            )));
            // The track currently being reported, shared by the two listeners
            // below: `player:position` carries no track id, so the mirror has
            // to be told which song the tick belongs to.
            let last_track: Arc<std::sync::Mutex<Option<i64>>> = Arc::default();
            {
                let mirror = position_mirror.clone();
                let current = last_track.clone();
                app.listen("player:position", move |event| {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) {
                        let track_id = current.lock().ok().and_then(|slot| *slot);
                        mirror.record(
                            track_id,
                            value
                                .get("ms")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0),
                            value
                                .get("duration_ms")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0),
                        );
                    }
                });
            }
            // Player events reach plugins by listening to the same Tauri event
            // the webview gets, so the player thread keeps no plugin coupling.
            // Both are coalesced by the runtime, so a 250 ms tick cannot flood
            // a guest.
            {
                let event_plugins = plugins.clone();
                // `listen` takes an Fn, so the last-seen track lives behind a
                // mutex rather than in the closure.
                let last_track = last_track.clone();
                app.listen("player:state", move |event| {
                    event_plugins.notify_event(HostEvent::PlayerStateChanged);
                    let track_id = serde_json::from_str::<serde_json::Value>(event.payload())
                        .ok()
                        .and_then(|state| state.get("track")?.get("id")?.as_i64());
                    if let Ok(mut previous) = last_track.lock() {
                        if *previous != track_id {
                            *previous = track_id;
                            event_plugins.notify_event(HostEvent::TrackChanged(track_id));
                        }
                    }
                });
            }
            {
                let scan_plugins = plugins.clone();
                // `library:changed` is the canonical signal and covers more
                // than a scan: importing an m3u's paths adds tracks and emits
                // only this one. A scan emits both, so nothing arrives twice.
                app.listen("library:changed", move |_| {
                    scan_plugins.notify_event(HostEvent::LibraryChanged);
                });
            }
            {
                let playlist_plugins = plugins.clone();
                app.listen("playlist:changed", move |event| {
                    let id = serde_json::from_str::<serde_json::Value>(event.payload())
                        .ok()
                        .and_then(|value| value.as_i64());
                    playlist_plugins.notify_event(HostEvent::PlaylistChanged(id));
                });
            }
            plugins.notify_event(HostEvent::Ready);

            // Restore persisted volume before the first play.
            if let Ok(Some(v)) = db.prefs_get("volume") {
                if let Ok(v) = v.parse::<f64>() {
                    if let Err(error) = player.send(player::Cmd::SetVolume(v)) {
                        log::warn!("could not restore player volume: {error}");
                    }
                }
            }

            app.manage(AppState {
                db,
                art,
                waveform,
                plugins,
                player,
                projectm_grants: Arc::new(std::sync::Mutex::new(Default::default())),
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
            commands::player::player_set_projectm_enabled,
            commands::player::player_take_projectm_pcm,
            commands::player::projectm_choose_folder,
            commands::player::projectm_restore_folder,
            commands::player::projectm_load_preset,
            commands::player::projectm_load_asset,
            commands::player::player_get_state,
            commands::player::queue_add,
            commands::player::queue_add_context,
            commands::player::queue_remove,
            commands::player::queue_clear,
            commands::player::queue_list,
            commands::playlists::playlist_list,
            commands::playlists::playlist_create,
            commands::playlists::playlist_rename,
            commands::playlists::playlist_delete,
            commands::playlists::playlists_set_order,
            commands::playlists::playlist_tracks,
            commands::playlists::playlist_tracks_multi,
            commands::playlists::track_playlist_ids,
            commands::playlists::playlist_add_tracks,
            commands::playlists::playlist_remove_tracks,
            commands::playlists::playlist_reorder,
            commands::playlists::playlist_add_paths,
            commands::playlists::playlist_import_m3u,
            commands::playlists::collection_list,
            commands::playlists::collection_set,
            commands::playlists::collection_rename,
            commands::playlists::collection_delete,
            commands::playlists::collection_set_collapsed,
            commands::plugins::plugin_list,
            commands::plugins::plugin_choose_install,
            commands::plugins::plugin_set_enabled,
            commands::plugins::plugin_set_setting,
            commands::plugins::plugin_uninstall,
            commands::plugins::plugin_grouping_migration,
            commands::plugins::plugin_restart_app,
            commands::plugins::plugin_approve_grants,
            commands::plugins::plugin_invoke,
            commands::misc::prefs_get_all,
            commands::misc::prefs_set,
            commands::misc::about_info,
            commands::misc::waveform_get,
            commands::misc::lastfm_artist_info,
            commands::misc::lastfm_album_info,
            commands::misc::lastfm_test_key,
            commands::misc::lyrics_for_track,
            commands::misc::autostart_get,
            commands::misc::autostart_set,
            commands::misc::open_cover_window,
            commands::misc::open_about_window,
            commands::update::update_check,
            commands::update::update_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
