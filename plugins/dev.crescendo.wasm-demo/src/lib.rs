//! Canonical sandboxed Wasm plugin example for Crescendo.
//!
//! Exercises the whole runtime surface a plugin author sees: host-rendered
//! live settings, a host-rendered menu action, namespaced storage, windowed
//! library reads, the coalesced event handlers, and command dispatch through
//! `plugin_invoke`. Build with `build.sh` and pack the result with the
//! `crescendo-plugin` tool.
//!
//! The manifest requests only `library.read` and `storage`, so the player,
//! playlist, and network commands below are also the example of what a
//! *denied* capability looks like from inside the sandbox: an ordinary error
//! variant, reported as data, not a trap.

use crescendo_plugin_sdk::{
    export_plugin, host, library, net, player, playlists, storage, Guest, LogLevel, StorageError,
};

struct Component;

export_plugin!(Component);

fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out
}

/// Reads the string value of `"key"` from a small flat JSON object. Inputs
/// arrive from the host's tests and settings UI; nested objects are not used.
fn json_string_field(input: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = input.find(&needle)? + needle.len();
    let rest = input[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

impl Guest for Component {
    fn init() -> Result<(), ()> {
        let settings = host::settings();
        host::log(
            LogLevel::Info,
            &format!("demo initialized with {} live settings", settings.len()),
        );
        Ok(())
    }

    fn invoke(command: String, input: String) -> Result<String, String> {
        match command.as_str() {
            "identity" => Ok(input),
            "settings" => {
                let settings = host::settings();
                let keys: Vec<String> = settings
                    .iter()
                    .map(|setting| format!("\"{}\"", json_escape(&setting.key)))
                    .collect();
                Ok(format!(
                    "{{\"count\":{},\"keys\":[{}]}}",
                    settings.len(),
                    keys.join(",")
                ))
            }
            "storage_roundtrip" => {
                let value = json_string_field(&input, "value").unwrap_or_default();
                match storage::set("demo-key", &value) {
                    Ok(()) => match storage::get("demo-key") {
                        Ok(Some(stored)) => {
                            Ok(format!("{{\"stored\":\"{}\"}}", json_escape(&stored)))
                        }
                        Ok(None) => Ok("{\"stored\":null}".into()),
                        Err(_) => Err("storage get failed".into()),
                    },
                    Err(StorageError::Denied) => Ok("{\"denied\":true}".into()),
                    Err(StorageError::QuotaExceeded) => Ok("{\"quota\":true}".into()),
                    Err(StorageError::InvalidKey) => Err("invalid storage key".into()),
                }
            }
            "library_summary" => match library::tracks(0, 50) {
                Ok(page) => {
                    let first = page
                        .tracks
                        .first()
                        .and_then(|track| track.title.clone())
                        .unwrap_or_default();
                    Ok(format!(
                        "{{\"total\":{},\"first_title\":\"{}\"}}",
                        page.total,
                        json_escape(&first)
                    ))
                }
                Err(_) => Ok("{\"denied\":true}".into()),
            },
            // Capability probes: each reports the outcome as text so a test
            // can tell "denied" from "worked" from "the host has no player".
            "player_state" => Ok(match player::get_state() {
                Ok(state) => format!(
                    "{{\"playing\":{},\"volume\":{}}}",
                    state.playing, state.volume
                ),
                Err(player::PlayerError::Denied) => "denied".into(),
                Err(player::PlayerError::Unavailable) => "unavailable".into(),
                Err(player::PlayerError::InvalidRequest) => "invalid".into(),
            }),
            "player_stop" => Ok(match player::stop() {
                Ok(()) => "ok".into(),
                Err(player::PlayerError::Denied) => "denied".into(),
                Err(player::PlayerError::Unavailable) => "unavailable".into(),
                Err(player::PlayerError::InvalidRequest) => "invalid".into(),
            }),
            // Enqueueing is the one player call that takes data from the
            // plugin, so it is also the example of the host validating what a
            // plugin hands it: an id that names no track is refused.
            "player_enqueue" => {
                let id = json_string_field(&input, "id")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                Ok(match player::enqueue(&[id]) {
                    Ok(()) => "ok".into(),
                    Err(player::PlayerError::Denied) => "denied".into(),
                    Err(player::PlayerError::Unavailable) => "unavailable".into(),
                    Err(player::PlayerError::InvalidRequest) => "invalid".into(),
                })
            }
            "playlists_all" => Ok(match playlists::all() {
                Ok(list) => format!("{{\"count\":{}}}", list.len()),
                Err(playlists::PlaylistError::Denied) => "denied".into(),
                Err(_) => "error".into(),
            }),
            "playlist_create" => {
                let name = json_string_field(&input, "name").unwrap_or_default();
                Ok(match playlists::create(&name) {
                    Ok(id) => format!("{{\"id\":{id}}}"),
                    Err(playlists::PlaylistError::Denied) => "denied".into(),
                    Err(_) => "error".into(),
                })
            }
            "net_fetch" => {
                let url = json_string_field(&input, "url").unwrap_or_default();
                Ok(match net::fetch("GET", &url, None) {
                    Ok(response) => format!("{{\"status\":{}}}", response.status),
                    Err(net::NetError::Denied) => "denied".into(),
                    Err(net::NetError::BlockedHost) => "blocked".into(),
                    Err(net::NetError::InvalidRequest) => "invalid".into(),
                    Err(net::NetError::TooLarge) => "too-large".into(),
                    Err(net::NetError::Transport(_)) => "transport".into(),
                })
            }
            "spin" => loop {},
            other => Err(format!("unknown command {other}")),
        }
    }

    fn on_settings_changed() {
        let settings = host::settings();
        host::log(
            LogLevel::Debug,
            &format!("settings changed; now {} values", settings.len()),
        );
    }

    // Event handlers are coalesced notifications, not a stream: the host may
    // drop duplicates under load, so each one re-reads what it needs instead
    // of accumulating state from the callbacks.
    fn on_ready() {
        host::log(LogLevel::Info, "host ready");
    }

    fn on_player_state_changed() {
        host::log(LogLevel::Debug, "player state changed");
    }

    fn on_track_changed(track_id: Option<u64>) {
        host::log(
            LogLevel::Debug,
            &match track_id {
                Some(id) => format!("track changed to {id}"),
                None => "playback stopped".to_string(),
            },
        );
    }

    fn on_library_changed() {
        host::log(LogLevel::Debug, "library changed");
    }

    fn on_playlist_changed(playlist_id: Option<u64>) {
        host::log(
            LogLevel::Debug,
            &match playlist_id {
                Some(id) => format!("playlist {id} changed"),
                None => "several playlists changed".to_string(),
            },
        );
    }
}
