//! Sandboxed Wasm runtime for Crescendo plugins (Stage 5).
//!
//! One dedicated thread owns a Wasmtime engine and one isolated component
//! instance per loaded Wasm plugin. Nothing here runs on the UI or player
//! threads. The components get no WASI: no filesystem, environment, process,
//! socket, clock, or database access. Instead the host links the
//! `crescendo:host` WIT interfaces (`plugin-sdk/wit/crescendo-plugin.wit`),
//! and every capability-gated call is denied unless the user approved the
//! matching manifest permission for this exact package.
//!
//! Plugin identity is bound to the store, never to a call parameter: host
//! imports receive `&mut StoreData` and cannot be pointed at another plugin.
//! Guest work is bounded by a memory limit, per-call fuel, an epoch deadline,
//! bounded input/output sizes, and a bounded request queue.

use super::bridge::{
    BridgePlayerCommand, BridgeRepeat, HostNotification, HostNotifierSlot, PlayerBridgeSlot,
};
use super::manifest::{PluginManifest, PluginSettingDeclaration};
use super::net;
use super::permissions::PluginPermission;
use crate::db::Db;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wasmtime::component::{bindgen, Component, HasSelf, Linker};
use wasmtime::{Config, Engine, ResourceLimiter, Store};

bindgen!({
    path: "../../plugin-sdk/wit",
    world: "crescendo-plugin",
});

/// Linear-memory ceiling per plugin instance.
pub const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Maximum `module.wasm` size the host will read.
pub const MAX_MODULE_BYTES: u64 = 64 * 1024 * 1024;
/// Bounded JSON payload sizes for `plugin_invoke`.
pub const MAX_INVOKE_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_INVOKE_OUTPUT_BYTES: usize = 1024 * 1024;
/// Per-plugin namespaced-storage limits.
pub const MAX_STORAGE_KEY_BYTES: usize = 256;
pub const MAX_STORAGE_VALUE_BYTES: usize = 256 * 1024;
pub const STORAGE_QUOTA_BYTES: i64 = 5 * 1024 * 1024;
/// Largest library page a plugin may request.
pub const MAX_LIBRARY_PAGE: i64 = 2000;
/// Bytes of a guest-supplied error string kept when a command fails.
pub const MAX_GUEST_ERROR_BYTES: usize = 4096;
/// Bytes charged per table element against the 64 MiB instance budget. A
/// table slot is a reference; this is deliberately the pointer size rather
/// than an exact accounting, because the point is a ceiling, not a tally.
const TABLE_ELEMENT_BYTES: usize = std::mem::size_of::<usize>();
/// Diagnostic-log entries kept per plugin, per session.
const MAX_DIAGNOSTIC_ENTRIES: usize = 64;
const MAX_DIAGNOSTIC_BYTES: usize = 512;
/// Pending dispatcher requests before callers get a busy error.
const MAX_PENDING_REQUESTS: usize = 16;
/// Session runtime failures before a plugin is marked unhealthy.
pub const UNHEALTHY_FAILURE_LIMIT: u32 = 3;
/// Startup-init failures (persisted) before a plugin auto-disables.
pub const STARTUP_FAILURE_AUTO_DISABLE: i64 = 2;

/// Tunable limits; the defaults are the shipped policy, tests use tighter
/// values to exercise fuel and epoch exhaustion.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeLimits {
    /// Fuel granted per guest call. Roughly instruction count.
    pub fuel: u64,
    /// Epoch ticks granted per guest call; the ticker advances 1 tick/ms.
    pub epoch_ticks: u64,
    /// How long a caller waits for the runtime's reply.
    pub invoke_timeout: Duration,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            fuel: 2_000_000_000,
            epoch_ticks: 10_000,
            invoke_timeout: Duration::from_secs(30),
        }
    }
}

/// Where the host keeps the plugin tree, so the runtime can ask for safe mode
/// when it auto-disables a plugin.
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    /// Written to request safe mode on the *next* start. The manager consumes
    /// it early in `start`, before this runtime exists, so a request made here
    /// can never be deleted by the same run that caused it.
    pub safe_mode_marker: PathBuf,
}

/// A loaded Wasm plugin the runtime should instantiate.
pub struct WasmPluginSpec {
    pub id: String,
    pub module_path: PathBuf,
    pub manifest: Arc<PluginManifest>,
}

/// Message sent to the runtime thread.
enum RuntimeMsg {
    Invoke {
        plugin_id: String,
        command: String,
        input: String,
        reply: mpsc::Sender<Result<String>>,
    },
    SettingsChanged {
        plugin_id: String,
    },
    /// Broadcast host events. Each is coalesced: while one is being delivered,
    /// further copies of the same event are dropped rather than queued, so a
    /// burst of player ticks cannot outrun the guests.
    Event(HostEvent),
    Shutdown,
}

/// A host event delivered to every ready plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Startup finished and every plugin is instantiated.
    Ready,
    /// Playback started, paused, stopped, or changed volume.
    PlayerStateChanged,
    /// The playing track changed; `None` means playback stopped.
    TrackChanged(Option<i64>),
    /// A scan committed library changes.
    LibraryChanged,
    /// A playlist changed; `None` means several did.
    PlaylistChanged(Option<i64>),
}

/// Runtime-side session state for one plugin, shared with the manager so
/// snapshots surface failures without talking to the thread.
#[derive(Debug, Default)]
struct PluginRuntimeState {
    ready: bool,
    unhealthy: bool,
    session_failures: u32,
    error: Option<String>,
    diagnostics: VecDeque<String>,
}

/// Per-plugin runtime information for snapshots.
#[derive(Debug, Clone, Default)]
pub struct RuntimeInfo {
    pub ready: bool,
    pub unhealthy: bool,
    pub error: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Default)]
struct RuntimeStates {
    plugins: Mutex<BTreeMap<String, PluginRuntimeState>>,
}

impl RuntimeStates {
    fn with_plugin<T>(
        &self,
        plugin_id: &str,
        body: impl FnOnce(&mut PluginRuntimeState) -> T,
    ) -> T {
        let mut plugins = self
            .plugins
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        body(plugins.entry(plugin_id.to_string()).or_default())
    }

    fn info(&self, plugin_id: &str) -> RuntimeInfo {
        self.with_plugin(plugin_id, |state| RuntimeInfo {
            ready: state.ready,
            unhealthy: state.unhealthy,
            error: state.error.clone(),
            diagnostics: state.diagnostics.iter().cloned().collect(),
        })
    }

    fn push_diagnostic(&self, plugin_id: &str, message: &str) {
        let entry = truncate_on_boundary(message, MAX_DIAGNOSTIC_BYTES);
        self.with_plugin(plugin_id, |state| {
            state.diagnostics.push_back(entry);
            while state.diagnostics.len() > MAX_DIAGNOSTIC_ENTRIES {
                state.diagnostics.pop_front();
            }
        });
    }

    fn mark_ready(&self, plugin_id: &str) {
        self.with_plugin(plugin_id, |state| {
            state.ready = true;
            state.error = None;
        });
    }

    /// A trapped, timed-out, or otherwise failed guest call. Repeated
    /// failures mark the plugin unhealthy for the rest of the session.
    fn record_failure(&self, plugin_id: &str, message: &str) {
        self.push_diagnostic(plugin_id, message);
        self.with_plugin(plugin_id, |state| {
            state.session_failures = state.session_failures.saturating_add(1);
            state.error = Some(message.to_string());
            if state.session_failures >= UNHEALTHY_FAILURE_LIMIT {
                state.unhealthy = true;
                let failures = state.session_failures;
                state.error = Some(format!(
                    "Marked unhealthy for this session after {failures} runtime failures; the last failure was: {message}"
                ));
            }
        });
    }
}

/// Cut guest text to a byte budget without splitting a character.
///
/// Guest strings are arbitrary UTF-8. Taking N *characters* can yield several
/// times N bytes, and `String::truncate` panics unless the index lands on a
/// character boundary — on the shared runtime thread that panic would take
/// every plugin down with it.
fn truncate_on_boundary(message: &str, max_bytes: usize) -> String {
    if message.len() <= max_bytes {
        return message.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_string()
}

/// Handle used by the manager (and the `plugin_invoke` Tauri command) to talk
/// to the runtime thread. Dropping it stops the thread.
pub struct WasmHandle {
    tx: mpsc::SyncSender<RuntimeMsg>,
    states: Arc<RuntimeStates>,
    thread: Mutex<Option<JoinHandle<()>>>,
    invoke_timeout: Duration,
}

impl WasmHandle {
    /// Start the runtime thread with the given plugins. Fails only when the
    /// OS cannot spawn a thread; individual plugin failures are reported
    /// through [`WasmHandle::runtime_info`] instead.
    pub fn spawn(
        specs: Vec<WasmPluginSpec>,
        db: Arc<Db>,
        limits: RuntimeLimits,
        player: Arc<PlayerBridgeSlot>,
        notifier: Arc<HostNotifierSlot>,
        paths: RuntimePaths,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(MAX_PENDING_REQUESTS);
        let states = Arc::new(RuntimeStates::default());
        let worker_states = states.clone();
        let invoke_timeout = limits.invoke_timeout;
        let thread = thread::Builder::new()
            .name("crescendo-plugin-wasm".into())
            .spawn(move || {
                run(
                    specs,
                    db,
                    limits,
                    rx,
                    worker_states,
                    player,
                    notifier,
                    paths,
                )
            })
            .context("could not spawn the Wasm plugin runtime thread")?;
        Ok(Self {
            tx,
            states,
            thread: Mutex::new(Some(thread)),
            invoke_timeout,
        })
    }

    /// Dispatch one command. Input and output are bounded JSON documents; a
    /// trap, timeout, or busy runtime surfaces as an error, never as a hang.
    pub fn invoke(&self, plugin_id: &str, command: &str, input: &str) -> Result<String> {
        if input.len() > MAX_INVOKE_INPUT_BYTES {
            bail!("plugin command input exceeds 1 MiB");
        }
        let (reply, receiver) = mpsc::channel();
        self.tx
            .try_send(RuntimeMsg::Invoke {
                plugin_id: plugin_id.to_string(),
                command: command.to_string(),
                input: input.to_string(),
                reply,
            })
            .map_err(|_| anyhow!("the Wasm plugin runtime is busy"))?;
        receiver
            .recv_timeout(self.invoke_timeout)
            .map_err(|_| anyhow!("the Wasm plugin runtime did not answer in time"))?
    }

    /// Notify one plugin that its live settings changed. Coalesced by the
    /// runtime thread; never blocks the caller.
    pub fn notify_settings_changed(&self, plugin_id: &str) {
        let _ = self.tx.try_send(RuntimeMsg::SettingsChanged {
            plugin_id: plugin_id.to_string(),
        });
    }

    /// Broadcast one host event. Never blocks: if the runtime queue is full
    /// the event is dropped, which is the intended backpressure — events are
    /// notifications, and a guest that missed one re-reads state on demand.
    pub fn notify_event(&self, event: HostEvent) {
        let _ = self.tx.try_send(RuntimeMsg::Event(event));
    }

    pub fn runtime_info(&self, plugin_id: &str) -> RuntimeInfo {
        self.states.info(plugin_id)
    }

    fn stop(&self) {
        let _ = self.tx.send(RuntimeMsg::Shutdown);
        if let Ok(mut guard) = self.thread.lock() {
            if let Some(thread) = guard.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for WasmHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Per-instance host state. This is the only place plugin identity lives, so
/// host imports can never act on a caller-supplied id.
struct StoreData {
    plugin_id: String,
    manifest: Arc<PluginManifest>,
    permissions: BTreeSet<PluginPermission>,
    db: Arc<Db>,
    states: Arc<RuntimeStates>,
    player: Arc<PlayerBridgeSlot>,
    notifier: Arc<HostNotifierSlot>,
    network_domains: Vec<String>,
    limiter: MemoryLimiter,
}

impl StoreData {
    fn new(
        spec: &WasmPluginSpec,
        db: Arc<Db>,
        states: Arc<RuntimeStates>,
        player: Arc<PlayerBridgeSlot>,
        notifier: Arc<HostNotifierSlot>,
    ) -> Result<Self> {
        let permissions = super::permissions::parse_permissions(&spec.manifest.permissions)?;
        Ok(Self {
            plugin_id: spec.id.clone(),
            manifest: spec.manifest.clone(),
            permissions,
            db,
            states,
            player,
            notifier,
            // Approved at grant time; the runtime re-reads them from the
            // manifest so a call can never widen its own allowlist.
            network_domains: spec.manifest.network_domains.clone(),
            limiter: MemoryLimiter::new(),
        })
    }

    /// Map a capability check onto the player interface's error type.
    fn control(
        &self,
        command: BridgePlayerCommand,
    ) -> std::result::Result<(), crescendo::host::player::PlayerError> {
        if self.require(PluginPermission::PlayerControl).is_err() {
            return Err(crescendo::host::player::PlayerError::Denied);
        }
        if matches!(command, BridgePlayerCommand::Seek { position_ms } if position_ms > crate::model::MAX_SEEK_MS)
        {
            return Err(crescendo::host::player::PlayerError::InvalidRequest);
        }
        let bridge = self
            .player
            .get()
            .ok_or(crescendo::host::player::PlayerError::Unavailable)?;
        bridge
            .control(command)
            .map_err(|_| crescendo::host::player::PlayerError::InvalidRequest)
    }

    /// Tell the shell a plugin changed a playlist, so the sidebar refreshes
    /// and `on-playlist-changed` reaches other plugins.
    fn announce_playlist(&self, playlist_id: Option<i64>) {
        self.notifier
            .announce(HostNotification::PlaylistChanged(playlist_id));
    }

    fn require_playlists(
        &self,
        permission: PluginPermission,
    ) -> std::result::Result<(), crescendo::host::playlists::PlaylistError> {
        self.require(permission)
            .map_err(|_| crescendo::host::playlists::PlaylistError::Denied)
    }

    fn require(&self, permission: PluginPermission) -> Result<()> {
        if self.permissions.contains(&permission) {
            Ok(())
        } else {
            bail!("the package was not granted the {permission} permission")
        }
    }
}

/// 64 MiB total linear-memory budget per plugin instance.
struct MemoryLimiter {
    remaining: usize,
}

impl MemoryLimiter {
    fn new() -> Self {
        Self {
            remaining: MAX_MEMORY_BYTES,
        }
    }
}

impl ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let growth = desired.saturating_sub(current);
        if growth > self.remaining {
            return Ok(false);
        }
        self.remaining -= growth;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        // A table need not declare a maximum, so "bounded by its maximum" is
        // not a bound at all: a component could grow one until the process
        // dies. Charge table growth to the same budget as linear memory,
        // counting a reference-sized slot per element.
        let growth = desired
            .saturating_sub(current)
            .saturating_mul(TABLE_ELEMENT_BYTES);
        if growth > self.remaining {
            return Ok(false);
        }
        self.remaining -= growth;
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Host interface implementations (the `crescendo:host` WIT imports).
// ---------------------------------------------------------------------------

fn setting_json_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `crescendo:host/host` — live settings read and the bounded diagnostic log.
impl crescendo::host::host::Host for StoreData {
    fn settings(&mut self) -> Vec<crescendo::host::host::Setting> {
        let stored = self
            .db
            .plugin_settings_get(&self.plugin_id)
            .unwrap_or_default();
        self.manifest
            .contributes
            .settings
            .iter()
            .map(|declaration: &PluginSettingDeclaration| {
                let value = stored
                    .get(declaration.key())
                    .filter(|value| declaration.validate_value(value).is_ok())
                    .cloned()
                    .unwrap_or_else(|| declaration.default_value());
                crescendo::host::host::Setting {
                    key: declaration.key().to_string(),
                    value: setting_json_to_string(&value),
                }
            })
            .collect()
    }

    fn log(&mut self, level: crescendo::host::host::LogLevel, message: String) {
        let level = match level {
            crescendo::host::host::LogLevel::Debug => "debug",
            crescendo::host::host::LogLevel::Info => "info",
            crescendo::host::host::LogLevel::Warn => "warn",
            crescendo::host::host::LogLevel::Error => "error",
        };
        let entry = format!("{level}: {message}");
        log::debug!("plugin {}: {entry}", self.plugin_id);
        self.states.push_diagnostic(&self.plugin_id, &entry);
    }
}

fn valid_storage_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_STORAGE_KEY_BYTES
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
}

/// `crescendo:host/storage` — namespaced key/value storage, gated by the
/// `storage` permission and a 5 MiB quota.
impl crescendo::host::storage::Host for StoreData {
    fn get(
        &mut self,
        key: String,
    ) -> std::result::Result<Option<String>, crescendo::host::storage::StorageError> {
        if self.require(PluginPermission::Storage).is_err() {
            return Err(crescendo::host::storage::StorageError::Denied);
        }
        if !valid_storage_key(&key) {
            return Err(crescendo::host::storage::StorageError::InvalidKey);
        }
        self.db
            .plugin_storage_get(&self.plugin_id, &key)
            .map_err(|_| crescendo::host::storage::StorageError::Denied)
    }

    fn set(
        &mut self,
        key: String,
        value: String,
    ) -> std::result::Result<(), crescendo::host::storage::StorageError> {
        if self.require(PluginPermission::Storage).is_err() {
            return Err(crescendo::host::storage::StorageError::Denied);
        }
        if !valid_storage_key(&key) || value.len() > MAX_STORAGE_VALUE_BYTES {
            return Err(crescendo::host::storage::StorageError::InvalidKey);
        }
        let usage = self
            .db
            .plugin_storage_usage(&self.plugin_id)
            .map_err(|_| crescendo::host::storage::StorageError::Denied)?;
        let existing = self
            .db
            .plugin_storage_get(&self.plugin_id, &key)
            .map_err(|_| crescendo::host::storage::StorageError::Denied)?;
        let delta = key.len() as i64 + value.len() as i64
            - existing.map_or(0, |old| key.len() as i64 + old.len() as i64);
        if usage + delta > STORAGE_QUOTA_BYTES {
            return Err(crescendo::host::storage::StorageError::QuotaExceeded);
        }
        self.db
            .plugin_storage_set(&self.plugin_id, &key, &value)
            .map_err(|_| crescendo::host::storage::StorageError::Denied)
    }

    fn delete(
        &mut self,
        key: String,
    ) -> std::result::Result<(), crescendo::host::storage::StorageError> {
        if self.require(PluginPermission::Storage).is_err() {
            return Err(crescendo::host::storage::StorageError::Denied);
        }
        if !valid_storage_key(&key) {
            return Err(crescendo::host::storage::StorageError::InvalidKey);
        }
        self.db
            .plugin_storage_delete(&self.plugin_id, &key)
            .map_err(|_| crescendo::host::storage::StorageError::Denied)
    }

    fn keys(&mut self) -> std::result::Result<Vec<String>, crescendo::host::storage::StorageError> {
        if self.require(PluginPermission::Storage).is_err() {
            return Err(crescendo::host::storage::StorageError::Denied);
        }
        self.db
            .plugin_storage_keys(&self.plugin_id)
            .map_err(|_| crescendo::host::storage::StorageError::Denied)
    }
}

/// `crescendo:host/library` — windowed, read-only library metadata.
impl crescendo::host::library::Host for StoreData {
    fn track_count(&mut self) -> std::result::Result<u64, crescendo::host::library::LibraryError> {
        if self.require(PluginPermission::LibraryRead).is_err() {
            return Err(crescendo::host::library::LibraryError::Denied);
        }
        self.db
            .plugin_library_count()
            .map(|count| count as u64)
            .map_err(|_| crescendo::host::library::LibraryError::Denied)
    }

    fn tracks(
        &mut self,
        offset: u64,
        limit: u32,
    ) -> std::result::Result<crescendo::host::library::Page, crescendo::host::library::LibraryError>
    {
        if self.require(PluginPermission::LibraryRead).is_err() {
            return Err(crescendo::host::library::LibraryError::Denied);
        }
        let limit = limit as i64;
        if limit < 1 || limit > MAX_LIBRARY_PAGE {
            return Err(crescendo::host::library::LibraryError::InvalidPage);
        }
        // The guest picks the offset. Casting a u64 past i64::MAX would wrap
        // negative, and SQLite reads a negative OFFSET as none at all, handing
        // back the first page instead of refusing the request.
        let Ok(offset) = i64::try_from(offset) else {
            return Err(crescendo::host::library::LibraryError::InvalidPage);
        };
        let include_paths = self.permissions.contains(&PluginPermission::LibraryPaths);
        let page = self
            .db
            .plugin_library_page(offset, limit, include_paths)
            .map_err(|_| crescendo::host::library::LibraryError::Denied)?;
        Ok(crescendo::host::library::Page {
            tracks: page
                .tracks
                .into_iter()
                .map(|track| crescendo::host::library::Track {
                    id: track.id as u64,
                    title: track.title,
                    artist: track.artist,
                    album: track.album,
                    album_artist: track.albumartist,
                    genre: track.genre,
                    grouping: track.grouping,
                    duration_ms: track.duration_ms as u64,
                    year: track.year.map(|value| value as u32),
                    track_no: track.track_no.map(|value| value as u32),
                    disc_no: track.disc_no.map(|value| value as u32),
                    path: track.path,
                })
                .collect(),
            total: page.total as u64,
        })
    }
}

/// `crescendo:host/player` — observation and bounded control. `player.read`
/// and `player.control` are separate: watching is not steering.
impl crescendo::host::player::Host for StoreData {
    fn get_state(
        &mut self,
    ) -> std::result::Result<crescendo::host::player::State, crescendo::host::player::PlayerError>
    {
        if self.require(PluginPermission::PlayerRead).is_err() {
            return Err(crescendo::host::player::PlayerError::Denied);
        }
        let bridge = self
            .player
            .get()
            .ok_or(crescendo::host::player::PlayerError::Unavailable)?;
        let state = bridge.state();
        Ok(crescendo::host::player::State {
            playing: state.playing,
            paused: state.paused,
            track_id: state.track_id.map(|id| id as u64),
            position_ms: state.position_ms,
            duration_ms: state.duration_ms,
            volume: state.volume,
            muted: state.muted,
            shuffle: state.shuffle,
            repeat: match state.repeat {
                Some(BridgeRepeat::All) => crescendo::host::player::RepeatMode::All,
                Some(BridgeRepeat::One) => crescendo::host::player::RepeatMode::One,
                _ => crescendo::host::player::RepeatMode::Off,
            },
        })
    }

    fn play_pause(&mut self) -> std::result::Result<(), crescendo::host::player::PlayerError> {
        self.control(BridgePlayerCommand::PlayPause)
    }

    fn stop(&mut self) -> std::result::Result<(), crescendo::host::player::PlayerError> {
        self.control(BridgePlayerCommand::Stop)
    }

    fn next(&mut self) -> std::result::Result<(), crescendo::host::player::PlayerError> {
        self.control(BridgePlayerCommand::Next)
    }

    fn previous(&mut self) -> std::result::Result<(), crescendo::host::player::PlayerError> {
        self.control(BridgePlayerCommand::Previous)
    }

    fn seek(
        &mut self,
        position_ms: u64,
    ) -> std::result::Result<(), crescendo::host::player::PlayerError> {
        self.control(BridgePlayerCommand::Seek { position_ms })
    }

    fn set_volume(
        &mut self,
        volume: f64,
    ) -> std::result::Result<(), crescendo::host::player::PlayerError> {
        if !(0.0..=1.0).contains(&volume) || !volume.is_finite() {
            return Err(crescendo::host::player::PlayerError::InvalidRequest);
        }
        self.control(BridgePlayerCommand::SetVolume { volume })
    }

    fn enqueue(
        &mut self,
        track_ids: Vec<u64>,
    ) -> std::result::Result<(), crescendo::host::player::PlayerError> {
        // Denial comes first. Validating the ids before the capability check
        // would answer "does track 4242 exist?" for a plugin that was never
        // granted the player at all.
        if self.require(PluginPermission::PlayerControl).is_err() {
            return Err(crescendo::host::player::PlayerError::Denied);
        }
        if track_ids.is_empty() || track_ids.len() > MAX_LIBRARY_PAGE as usize {
            return Err(crescendo::host::player::PlayerError::InvalidRequest);
        }
        let track_ids: Vec<i64> = track_ids.into_iter().map(|id| id as i64).collect();
        // The queue holds concrete ids and nothing downstream re-checks them,
        // so an unknown id would otherwise sit in the user's queue resolving
        // to no track at all.
        if !self.db.track_ids_all_exist(&track_ids).unwrap_or(false) {
            return Err(crescendo::host::player::PlayerError::InvalidRequest);
        }
        self.control(BridgePlayerCommand::Enqueue { track_ids })
    }
}

/// `crescendo:host/playlists` — reads and the bounded mutations. Deleting a
/// playlist is deliberately not exposed in v1.
impl crescendo::host::playlists::Host for StoreData {
    fn all(
        &mut self,
    ) -> std::result::Result<
        Vec<crescendo::host::playlists::Info>,
        crescendo::host::playlists::PlaylistError,
    > {
        self.require_playlists(PluginPermission::PlaylistsRead)?;
        let playlists = self
            .db
            .playlist_list()
            .map_err(|_| crescendo::host::playlists::PlaylistError::InvalidRequest)?;
        Ok(playlists
            .into_iter()
            .map(|playlist| crescendo::host::playlists::Info {
                id: playlist.id as u64,
                name: playlist.name,
                track_count: playlist.track_count.max(0) as u64,
            })
            .collect())
    }

    fn tracks(
        &mut self,
        playlist_id: u64,
        offset: u64,
        limit: u32,
    ) -> std::result::Result<Vec<u64>, crescendo::host::playlists::PlaylistError> {
        self.require_playlists(PluginPermission::PlaylistsRead)?;
        let limit = limit as usize;
        if limit == 0 || limit > MAX_LIBRARY_PAGE as usize {
            return Err(crescendo::host::playlists::PlaylistError::InvalidRequest);
        }
        if !self.db.playlist_exists(playlist_id as i64).unwrap_or(false) {
            return Err(crescendo::host::playlists::PlaylistError::NotFound);
        }
        // Same wrap as the library page: a u64 offset past i64::MAX would go
        // negative and SQLite would ignore it, returning the first page.
        let Ok(offset) = i64::try_from(offset) else {
            return Err(crescendo::host::playlists::PlaylistError::InvalidRequest);
        };
        let ids = self
            .db
            .playlist_track_ids_page(playlist_id as i64, offset, limit as i64)
            .map_err(|_| crescendo::host::playlists::PlaylistError::NotFound)?;
        Ok(ids.into_iter().map(|id| id as u64).collect())
    }

    fn create(
        &mut self,
        name: String,
    ) -> std::result::Result<u64, crescendo::host::playlists::PlaylistError> {
        self.require_playlists(PluginPermission::PlaylistsWrite)?;
        if !valid_playlist_name(&name) {
            return Err(crescendo::host::playlists::PlaylistError::InvalidRequest);
        }
        let id = self
            .db
            .playlist_create(&name)
            .map_err(|_| crescendo::host::playlists::PlaylistError::InvalidRequest)?;
        self.announce_playlist(Some(id));
        Ok(id as u64)
    }

    fn rename(
        &mut self,
        playlist_id: u64,
        name: String,
    ) -> std::result::Result<(), crescendo::host::playlists::PlaylistError> {
        self.require_playlists(PluginPermission::PlaylistsWrite)?;
        if !valid_playlist_name(&name) {
            return Err(crescendo::host::playlists::PlaylistError::InvalidRequest);
        }
        // Checked, so the playlist cannot be deleted between the test and the
        // write — the same reason `add_tracks` uses the checked form.
        self.db
            .playlist_rename_checked(playlist_id as i64, &name)
            .map_err(|_| crescendo::host::playlists::PlaylistError::NotFound)?;
        self.announce_playlist(Some(playlist_id as i64));
        Ok(())
    }

    fn add_tracks(
        &mut self,
        playlist_id: u64,
        track_ids: Vec<u64>,
    ) -> std::result::Result<(), crescendo::host::playlists::PlaylistError> {
        self.require_playlists(PluginPermission::PlaylistsWrite)?;
        if track_ids.is_empty() || track_ids.len() > MAX_LIBRARY_PAGE as usize {
            return Err(crescendo::host::playlists::PlaylistError::InvalidRequest);
        }
        let ids: Vec<i64> = track_ids.into_iter().map(|id| id as i64).collect();
        // Guest ids are arbitrary numbers; the checked form refuses unknown
        // playlists and tracks rather than persisting rows pointing at nothing.
        self.db
            .playlist_add_tracks_checked(playlist_id as i64, &ids)
            .map_err(|_| crescendo::host::playlists::PlaylistError::NotFound)?;
        self.announce_playlist(Some(playlist_id as i64));
        Ok(())
    }

    fn remove_positions(
        &mut self,
        playlist_id: u64,
        positions: Vec<u64>,
    ) -> std::result::Result<(), crescendo::host::playlists::PlaylistError> {
        self.require_playlists(PluginPermission::PlaylistsWrite)?;
        if positions.is_empty() || positions.len() > MAX_LIBRARY_PAGE as usize {
            return Err(crescendo::host::playlists::PlaylistError::InvalidRequest);
        }
        let positions: Vec<i64> = positions
            .into_iter()
            .map(|value| i64::try_from(value).unwrap_or(i64::MAX))
            .collect();
        self.db
            .playlist_remove_tracks_checked(playlist_id as i64, &positions)
            .map_err(|_| crescendo::host::playlists::PlaylistError::NotFound)?;
        self.announce_playlist(Some(playlist_id as i64));
        Ok(())
    }
}

/// `crescendo:host/net` — outbound HTTPS, allowlisted and revalidated per hop.
impl crescendo::host::net::Host for StoreData {
    fn fetch(
        &mut self,
        method: String,
        url: String,
        body: Option<String>,
    ) -> std::result::Result<crescendo::host::net::Response, crescendo::host::net::NetError> {
        if self.require(PluginPermission::NetworkHttp).is_err() {
            return Err(crescendo::host::net::NetError::Denied);
        }
        // Check before dialling so an unapproved or private target is refused
        // with the specific reason, not a generic transport failure.
        if let Err(rejection) = net::check_url(&url, &self.network_domains) {
            return Err(match rejection {
                net::NetRejection::BlockedHost(reason) => {
                    self.states.push_diagnostic(&self.plugin_id, &reason);
                    crescendo::host::net::NetError::BlockedHost
                }
                net::NetRejection::InvalidRequest(reason) => {
                    self.states.push_diagnostic(&self.plugin_id, &reason);
                    crescendo::host::net::NetError::InvalidRequest
                }
            });
        }
        match net::fetch(&method, &url, body.as_deref(), &self.network_domains) {
            Ok(response) => Ok(crescendo::host::net::Response {
                status: response.status,
                body: response.body,
            }),
            Err(error) => {
                let message = format!("{error:#}");
                self.states.push_diagnostic(&self.plugin_id, &message);
                if message.contains("1 MiB limit") {
                    Err(crescendo::host::net::NetError::TooLarge)
                } else if message.contains("approved domain")
                    || message.contains("non-public")
                    || message.contains("plain http")
                {
                    Err(crescendo::host::net::NetError::BlockedHost)
                } else if message.contains("support GET and POST") || message.contains("256 KiB") {
                    Err(crescendo::host::net::NetError::InvalidRequest)
                } else {
                    Err(crescendo::host::net::NetError::Transport(message))
                }
            }
        }
    }
}

/// Playlist names are host-rendered text; keep them short and printable.
fn valid_playlist_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed.len() <= 200 && !name.chars().any(char::is_control)
}

// ---------------------------------------------------------------------------
// Runtime thread.
// ---------------------------------------------------------------------------

struct PluginInstance {
    instance: CrescendoPlugin,
    store: Store<StoreData>,
}

fn run(
    specs: Vec<WasmPluginSpec>,
    db: Arc<Db>,
    limits: RuntimeLimits,
    rx: mpsc::Receiver<RuntimeMsg>,
    states: Arc<RuntimeStates>,
    player: Arc<PlayerBridgeSlot>,
    notifier: Arc<HostNotifierSlot>,
    paths: RuntimePaths,
) {
    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    let engine = match Engine::new(&config) {
        Ok(engine) => engine,
        Err(error) => {
            for spec in &specs {
                states.record_failure(
                    &spec.id,
                    &format!("could not start the Wasm engine: {error}"),
                );
            }
            return;
        }
    };

    // The epoch ticker advances engine deadlines from its own thread so guest
    // code cannot starve the host, not even with an infinite loop.
    let ticker_stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let engine = engine.clone();
        let stop = ticker_stop.clone();
        thread::Builder::new()
            .name("crescendo-plugin-wasm-epoch".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    engine.increment_epoch();
                    thread::sleep(Duration::from_millis(1));
                }
            })
            .ok()
    };

    let runtime = RuntimeThread {
        engine: &engine,
        linker: build_linker(&engine),
        plugins: BTreeMap::new(),
        db,
        limits,
        states,
        player,
        notifier,
        paths,
    };
    runtime.main(rx, specs);

    ticker_stop.store(true, Ordering::Relaxed);
    if let Some(ticker) = ticker {
        let _ = ticker.join();
    }
}

/// Compile and link a component without running any guest code.
///
/// The package validator can tell that `module.wasm` exists and is the right
/// size; it cannot tell that the bytes are a component this host can load. An
/// update promotes over a working copy, so without this a structurally valid
/// package carrying a broken module would replace a working plugin and the
/// only rollback copy would be deleted before anything discovered the problem.
///
/// This parses the component and resolves its imports and exports against the
/// real linker. It stops short of `init`: instantiating would run guest code,
/// and a plugin that fails at `init` is the case the runtime already handles
/// by reporting and eventually auto-disabling it.
pub fn preflight_component(module_path: &Path) -> Result<(), String> {
    let bytes = read_module(module_path).map_err(|error| format!("{error:#}"))?;
    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    let engine = Engine::new(&config)
        .map_err(|error| format!("could not start the Wasm engine: {error}"))?;
    let component = Component::new(&engine, bytes)
        .map_err(|error| format!("module.wasm is not a valid component: {error}"))?;
    let linker = build_linker(&engine);
    let instance_pre = linker
        .instantiate_pre(&component)
        .map_err(|error| format!("module.wasm does not match this runtime's world: {error}"))?;
    CrescendoPluginPre::new(instance_pre)
        .map(|_| ())
        .map_err(|error| format!("module.wasm does not export the plugin interface: {error}"))
}

fn build_linker(engine: &Engine) -> Linker<StoreData> {
    let mut linker: Linker<StoreData> = Linker::new(engine);
    // A component imports only what its code uses, so every interface is
    // always linked; the per-call permission checks do the gating. Missing one
    // here does not deny it — it makes any component importing it fail to
    // instantiate at all.
    // `HasSelf` gives the host traits a `&mut StoreData` view of the store.
    let _ = crescendo::host::host::add_to_linker::<StoreData, HasSelf<StoreData>>(
        &mut linker,
        |data| data,
    );
    let _ = crescendo::host::storage::add_to_linker::<StoreData, HasSelf<StoreData>>(
        &mut linker,
        |data| data,
    );
    let _ = crescendo::host::library::add_to_linker::<StoreData, HasSelf<StoreData>>(
        &mut linker,
        |data| data,
    );
    let _ = crescendo::host::player::add_to_linker::<StoreData, HasSelf<StoreData>>(
        &mut linker,
        |data| data,
    );
    let _ = crescendo::host::playlists::add_to_linker::<StoreData, HasSelf<StoreData>>(
        &mut linker,
        |data| data,
    );
    let _ =
        crescendo::host::net::add_to_linker::<StoreData, HasSelf<StoreData>>(&mut linker, |data| {
            data
        });
    linker
}

struct RuntimeThread<'a> {
    engine: &'a Engine,
    linker: Linker<StoreData>,
    plugins: BTreeMap<String, PluginInstance>,
    db: Arc<Db>,
    limits: RuntimeLimits,
    states: Arc<RuntimeStates>,
    player: Arc<PlayerBridgeSlot>,
    notifier: Arc<HostNotifierSlot>,
    paths: RuntimePaths,
}

impl RuntimeThread<'_> {
    fn main(mut self, rx: mpsc::Receiver<RuntimeMsg>, specs: Vec<WasmPluginSpec>) {
        for spec in specs {
            self.instantiate(&spec);
        }
        // Backlog of drained messages while coalescing notifications.
        let mut backlog: VecDeque<RuntimeMsg> = VecDeque::new();
        loop {
            let Some(message) = backlog.pop_front().or_else(|| rx.recv().ok()) else {
                break; // every handle was dropped
            };
            match message {
                RuntimeMsg::Shutdown => break,
                RuntimeMsg::Invoke {
                    plugin_id,
                    command,
                    input,
                    reply,
                } => {
                    let result = self.dispatch_invoke(&plugin_id, &command, &input);
                    let _ = reply.send(result);
                }
                RuntimeMsg::SettingsChanged { plugin_id } => {
                    // Coalesce: drain everything already queued and drop
                    // further notifications for the same plugin.
                    while let Ok(next) = rx.try_recv() {
                        match next {
                            RuntimeMsg::SettingsChanged {
                                plugin_id: other, ..
                            } if other == plugin_id => continue,
                            other => backlog.push_back(other),
                        }
                    }
                    self.dispatch_settings_changed(&plugin_id);
                }
                RuntimeMsg::Event(event) => {
                    while let Ok(next) = rx.try_recv() {
                        match next {
                            RuntimeMsg::Event(other) if other == event => continue,
                            other => backlog.push_back(other),
                        }
                    }
                    self.dispatch_event(&event);
                }
            }
        }
    }

    /// Deliver one event to every ready plugin. A guest that traps or runs out
    /// of fuel here is recorded like any other failure and skipped; one bad
    /// plugin never stops the others from being notified.
    fn dispatch_event(&mut self, event: &HostEvent) {
        let ids: Vec<String> = self.plugins.keys().cloned().collect();
        for plugin_id in ids {
            // An unhealthy plugin is done for the session. Invoke and settings
            // delivery already refuse it; without the same check here every
            // track change would keep paying its deadline and adding failures.
            let info = self.states.info(&plugin_id);
            if !info.ready || info.unhealthy {
                continue;
            }
            let Some(plugin) = self.plugins.get_mut(&plugin_id) else {
                continue;
            };
            if let Err(error) = plugin.store.set_fuel(self.limits.fuel) {
                self.states
                    .record_failure(&plugin_id, &format!("could not arm fuel: {error:#}"));
                continue;
            }
            plugin.store.set_epoch_deadline(self.limits.epoch_ticks);
            let commands = plugin.instance.crescendo_plugin_commands();
            let outcome = match event {
                HostEvent::Ready => commands.call_on_ready(&mut plugin.store),
                HostEvent::PlayerStateChanged => {
                    commands.call_on_player_state_changed(&mut plugin.store)
                }
                HostEvent::TrackChanged(track_id) => {
                    commands.call_on_track_changed(&mut plugin.store, track_id.map(|id| id as u64))
                }
                HostEvent::LibraryChanged => commands.call_on_library_changed(&mut plugin.store),
                HostEvent::PlaylistChanged(playlist_id) => commands
                    .call_on_playlist_changed(&mut plugin.store, playlist_id.map(|id| id as u64)),
            };
            if let Err(error) = outcome {
                self.states
                    .record_failure(&plugin_id, &format!("{event:?} handler failed: {error}"));
            }
        }
    }

    fn instantiate(&mut self, spec: &WasmPluginSpec) {
        match self.try_instantiate(spec) {
            Ok(()) => {}
            Err(message) => self.record_init_failure(spec, message),
        }
    }

    fn try_instantiate(&mut self, spec: &WasmPluginSpec) -> Result<(), String> {
        let bytes = read_module(&spec.module_path).map_err(|error| format!("{error:#}"))?;
        let component = Component::new(self.engine, bytes)
            .map_err(|error| format!("module.wasm is not a valid component: {error}"))?;
        let mut store = Store::new(
            self.engine,
            StoreData::new(
                spec,
                self.db.clone(),
                self.states.clone(),
                self.player.clone(),
                self.notifier.clone(),
            )
            .map_err(|error| format!("{error:#}"))?,
        );
        store.limiter(|data| &mut data.limiter);
        store
            .set_fuel(self.limits.fuel)
            .map_err(|error| format!("could not arm fuel: {error}"))?;
        store.set_epoch_deadline(self.limits.epoch_ticks);
        let instance = CrescendoPlugin::instantiate(&mut store, &component, &self.linker)
            .map_err(|error| format!("could not instantiate the component: {error}"))?;
        instance
            .crescendo_plugin_commands()
            .call_init(&mut store)
            .map_err(|error| format!("init trapped: {error}"))
            .and_then(|result| result.map_err(|()| "init returned an error".to_string()))?;

        self.plugins
            .insert(spec.id.clone(), PluginInstance { instance, store });
        self.states.mark_ready(&spec.id);
        // `mark_ready` clears only this session's view. The persisted count is
        // what decides auto-disable, and it must mean *consecutive* failures:
        // without this a plugin that failed once, started cleanly, and failed
        // again months later would be disabled as though it had never worked.
        // It is also what Settings shows, so a stale error would outlive the
        // problem it described.
        if self
            .db
            .plugin_state_get(&spec.id)
            .ok()
            .flatten()
            .is_some_and(|state| state.failure_count > 0 || state.last_error.is_some())
        {
            let _ = self.db.plugin_record_error(&spec.id, None);
        }
        Ok(())
    }

    /// Startup-initialization failure. The plugin stays installed; repeated
    /// failures (persisted via `failure_count`) auto-disable it so a broken
    /// package can never wedge every startup.
    fn record_init_failure(&mut self, spec: &WasmPluginSpec, message: String) {
        log::warn!("plugin {} failed to initialize: {message}", spec.id);
        let prior_failures = self
            .db
            .plugin_state_get(&spec.id)
            .map(|state| state.map_or(0, |state| state.failure_count))
            .unwrap_or(0);
        let message = if prior_failures + 1 >= STARTUP_FAILURE_AUTO_DISABLE {
            // The plan pairs auto-disable with safe mode: a package that fails
            // to initialize twice means the next launch should be cautious,
            // not merely short one plugin. The manager consumes this marker at
            // the start of the next run, so safe mode lasts exactly one run.
            if let Err(error) = std::fs::write(&self.paths.safe_mode_marker, b"wasm-init-failure") {
                log::warn!("could not arm plugin safe mode: {error}");
            }
            let disabled = self
                .db
                .plugin_set_desired_enabled(&spec.id, false)
                .and_then(|_| {
                    self.db.plugin_record_error(
                        &spec.id,
                        Some(&format!(
                            "Disabled after repeated startup failures; the last failure was: {message}"
                        )),
                    )
                })
                .is_ok();
            if disabled {
                format!("Disabled after repeated startup failures; the last failure was: {message}")
            } else {
                message
            }
        } else {
            let _ = self
                .db
                .plugin_record_error_preserving_state(&spec.id, &message);
            message
        };
        self.states.record_failure(&spec.id, &message);
    }

    fn dispatch_invoke(&mut self, plugin_id: &str, command: &str, input: &str) -> Result<String> {
        let Some(plugin) = self.plugins.get_mut(plugin_id) else {
            bail!("plugin {plugin_id} has no running Wasm instance");
        };
        let info = self.states.info(plugin_id);
        if info.unhealthy {
            bail!(
                "plugin {plugin_id} is unhealthy for this session: {}",
                info.error.as_deref().unwrap_or("repeated runtime failures")
            );
        }
        if !info.ready {
            bail!(
                "plugin {plugin_id} failed to initialize: {}",
                info.error.as_deref().unwrap_or("unknown startup failure")
            );
        }
        plugin
            .store
            .set_fuel(self.limits.fuel)
            .map_err(|error| anyhow!("could not arm fuel: {error}"))?;
        plugin.store.set_epoch_deadline(self.limits.epoch_ticks);
        let commands = plugin.instance.crescendo_plugin_commands();
        let outcome = commands.call_invoke(&mut plugin.store, command, input);
        match outcome {
            Ok(Ok(output)) => {
                if output.len() > MAX_INVOKE_OUTPUT_BYTES {
                    self.states.record_failure(
                        plugin_id,
                        "plugin command output exceeded 1 MiB and was discarded",
                    );
                    bail!("plugin command output exceeds 1 MiB");
                }
                Ok(output)
            }
            Ok(Err(message)) => {
                // A guest error is guest-controlled output too: bound it like
                // a success, or a plugin can return a megabyte of text that
                // travels all the way out through the command boundary.
                let message = truncate_on_boundary(&message, MAX_GUEST_ERROR_BYTES);
                let entry = format!("command {command:?} failed: {message}");
                self.states.push_diagnostic(plugin_id, &entry);
                bail!("plugin command {command:?} failed: {message}")
            }
            Err(error) => {
                let entry = format!("command {command:?} trapped or timed out: {error:#}");
                self.states.record_failure(plugin_id, &entry);
                bail!("{entry}")
            }
        }
    }

    fn dispatch_settings_changed(&mut self, plugin_id: &str) {
        let Some(plugin) = self.plugins.get_mut(plugin_id) else {
            return;
        };
        let info = self.states.info(plugin_id);
        if info.unhealthy || !info.ready {
            return;
        }
        if let Err(error) = plugin.store.set_fuel(self.limits.fuel) {
            self.states
                .record_failure(plugin_id, &format!("could not arm fuel: {error:#}"));
            return;
        }
        plugin.store.set_epoch_deadline(self.limits.epoch_ticks);
        let commands = plugin.instance.crescendo_plugin_commands();
        if let Err(error) = commands.call_on_settings_changed(&mut plugin.store) {
            self.states.record_failure(
                plugin_id,
                &format!("on-settings-changed trapped: {error:#}"),
            );
        }
    }
}

fn read_module(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("module.wasm is not a regular file");
    }
    if metadata.len() == 0 || metadata.len() > MAX_MODULE_BYTES {
        bail!("module.wasm must be between 1 byte and 64 MiB");
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_MODULE_BYTES {
        bail!("module.wasm exceeds the 64 MiB limit");
    }
    Ok(bytes)
}

pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_seek_is_checked_before_bridge_dispatch() {
        use crate::plugins::{BridgePlayerState, PlayerBridge};
        use crescendo::host::player::{Host, PlayerError};
        struct RecordingBridge(Mutex<Vec<u64>>);
        impl PlayerBridge for RecordingBridge {
            fn state(&self) -> BridgePlayerState {
                BridgePlayerState::default()
            }
            fn control(&self, command: BridgePlayerCommand) -> Result<(), String> {
                if let BridgePlayerCommand::Seek { position_ms } = command {
                    self.0.lock().unwrap().push(position_ms);
                }
                Ok(())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut manifest: PluginManifest = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/dev.crescendo.wasm-demo/manifest.json"
        )))
        .unwrap();
        manifest.permissions = vec!["player.control".into()];
        let spec = WasmPluginSpec {
            id: manifest.id.clone(),
            module_path: dir.path().join("unused.wasm"),
            manifest: Arc::new(manifest),
        };
        let slot = Arc::new(PlayerBridgeSlot::new());
        let bridge = Arc::new(RecordingBridge(Mutex::new(Vec::new())));
        slot.install(bridge.clone());
        let mut data = StoreData::new(
            &spec,
            Arc::new(Db::open(&dir.path().join("test.db")).unwrap()),
            Arc::new(RuntimeStates::default()),
            slot,
            Arc::new(HostNotifierSlot::new()),
        )
        .unwrap();
        for ms in [crate::model::MAX_SEEK_MS + 1, u64::MAX] {
            assert!(matches!(data.seek(ms), Err(PlayerError::InvalidRequest)));
        }
        assert!(bridge.0.lock().unwrap().is_empty());
        data.seek(180_000).unwrap();
        assert_eq!(*bridge.0.lock().unwrap(), vec![180_000]);
        data.permissions.clear();
        assert!(matches!(data.seek(u64::MAX), Err(PlayerError::Denied)));
    }

    #[test]
    fn guest_text_is_cut_on_a_character_boundary_not_mid_codepoint() {
        // Multibyte throughout: 512 characters is far more than 512 bytes, and
        // a naive truncate at byte 512 would land mid-character and panic,
        // taking the shared runtime thread with it.
        let message = "。".repeat(1000);
        let cut = truncate_on_boundary(&message, MAX_DIAGNOSTIC_BYTES);
        assert!(cut.len() <= MAX_DIAGNOSTIC_BYTES);
        assert!(cut.chars().all(|c| c == '。'), "cut mid-character");
        // Ascii still fills the budget exactly.
        assert_eq!(truncate_on_boundary(&"a".repeat(1000), 10).len(), 10);
        // Short input is untouched.
        assert_eq!(truncate_on_boundary("hi", 512), "hi");
    }

    #[test]
    fn table_growth_is_charged_against_the_instance_budget() {
        let mut limiter = MemoryLimiter::new();
        // A table with no declared maximum must not be able to grow without
        // bound; it draws on the same 64 MiB budget as linear memory.
        let huge = MAX_MEMORY_BYTES / TABLE_ELEMENT_BYTES + 1;
        assert!(!limiter.table_growing(0, huge, None).unwrap());
        assert!(limiter.table_growing(0, 1024, None).unwrap());
    }

    #[test]
    fn memory_limiter_enforces_the_64_mib_budget() {
        let mut limiter = MemoryLimiter::new();
        assert!(limiter.memory_growing(0, 1024 * 1024, None).unwrap());
        assert_eq!(limiter.remaining, MAX_MEMORY_BYTES - 1024 * 1024);
        // A growth request beyond the remaining budget is refused, and the
        // refusal must not consume the budget.
        let remaining = limiter.remaining;
        assert!(!limiter.memory_growing(0, remaining + 1, None).unwrap());
        assert_eq!(limiter.remaining, remaining);
        // The full budget fits exactly.
        let mut fresh = MemoryLimiter::new();
        assert!(fresh.memory_growing(0, MAX_MEMORY_BYTES, None).unwrap());
        assert_eq!(fresh.remaining, 0);
        assert!(!fresh.memory_growing(0, MAX_MEMORY_BYTES + 1, None).unwrap());
    }

    #[test]
    fn storage_keys_are_bounded_lowercase_identifiers() {
        assert!(valid_storage_key("demo-key"));
        assert!(valid_storage_key("a.b_c-1"));
        assert!(!valid_storage_key(""));
        assert!(!valid_storage_key("UPPER"));
        assert!(!valid_storage_key("with space"));
        assert!(!valid_storage_key("with/slash"));
        assert!(!valid_storage_key(&"k".repeat(MAX_STORAGE_KEY_BYTES + 1)));
    }

    #[test]
    fn runtime_states_mark_failures_unhealthy_after_the_threshold() {
        let states = RuntimeStates::default();
        for index in 0..UNHEALTHY_FAILURE_LIMIT {
            states.record_failure("p", &format!("failure {index}"));
            assert_eq!(
                states.info("p").unhealthy,
                index == UNHEALTHY_FAILURE_LIMIT - 1
            );
        }
        let info = states.info("p");
        assert!(info.unhealthy);
        assert!(info.error.as_deref().unwrap_or("").contains("unhealthy"));
        // The bounded diagnostic log keeps entries in order.
        assert_eq!(info.diagnostics[0], "failure 0");
    }

    #[test]
    fn runtime_states_coarsen_and_bound_diagnostics() {
        let states = RuntimeStates::default();
        for index in 0..MAX_DIAGNOSTIC_ENTRIES + 8 {
            states.push_diagnostic("p", &format!("entry {index}"));
        }
        let diagnostics = states.info("p").diagnostics;
        assert_eq!(diagnostics.len(), MAX_DIAGNOSTIC_ENTRIES);
        assert_eq!(*diagnostics.last().unwrap(), "entry 71");
    }
}
