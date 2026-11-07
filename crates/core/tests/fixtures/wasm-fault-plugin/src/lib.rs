//! A plugin that fails on request, for testing how the runtime recovers:
//! keeps a counter in memory, and can trap in a command or an event handler or
//! refuse its next `init`. The demo plugin is the example to read instead.

use crescendo_plugin_sdk::{export_plugin, host, storage, Guest, LogLevel};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const FAIL_INIT_KEY: &str = "fail-init";
const VALUE_KEY: &str = "value";

static COUNT: AtomicU32 = AtomicU32::new(0);
static SPIN_ON_EVENT: AtomicBool = AtomicBool::new(false);

struct Component;

export_plugin!(Component);

impl Guest for Component {
    fn init() -> Result<(), ()> {
        if matches!(storage::get(FAIL_INIT_KEY), Ok(Some(_))) {
            return Err(());
        }
        Ok(())
    }

    fn invoke(command: String, input: String) -> Result<String, String> {
        let stored = |result: Result<(), storage::StorageError>| {
            result
                .map(|()| "ok".to_string())
                .map_err(|_| "storage failed".to_string())
        };
        match command.as_str() {
            "count" => Ok((COUNT.fetch_add(1, Ordering::Relaxed) + 1).to_string()),
            "spin" => loop {},
            "arm_event_spin" => {
                SPIN_ON_EVENT.store(true, Ordering::Relaxed);
                Ok("ok".into())
            }
            "arm_init_failure" => stored(storage::set(FAIL_INIT_KEY, "1")),
            "store" => stored(storage::set(VALUE_KEY, &input)),
            "load" => match storage::get(VALUE_KEY) {
                Ok(value) => Ok(value.unwrap_or_default()),
                Err(_) => Err("storage failed".into()),
            },
            other => Err(format!("unknown command {other}")),
        }
    }

    fn on_settings_changed() {}

    fn on_ready() {
        host::log(LogLevel::Info, "fault plugin saw on-ready");
    }

    fn on_player_state_changed() {}

    fn on_track_changed(_track_id: Option<u64>) {}

    fn on_library_changed() {
        if SPIN_ON_EVENT.load(Ordering::Relaxed) {
            loop {}
        }
    }

    fn on_playlist_changed(_playlist_id: Option<u64>) {}
}
