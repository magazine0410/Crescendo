//! The shell's implementation of the core plugin host bridge.
//!
//! `crescendo-core` declares what the Wasm runtime needs from the player but
//! cannot own it: the player is a GStreamer thread that lives here. This
//! adapter translates the bounded command set in `crescendo_core::plugins`
//! into the player's own `Cmd`s, and nothing else — there is no passthrough,
//! so a capability can only reach the operations the WIT interface names.
//!
//! Capability checks happen in the runtime, against the identity bound to the
//! calling store. By the time a call arrives here it is already authorised.

use crate::player::{Cmd, PlayerHandle, Status};
use crescendo_core::plugins::{
    BridgePlayerCommand, BridgePlayerState, BridgeRepeat, HostNotification, HostNotifier,
    PlayerBridge,
};
use crescendo_core::playqueue::RepeatMode;
use std::sync::{Arc, Mutex};

/// Playback progress, mirrored from the `player:position` event.
///
/// `PlayerState` carries no position — it is emitted separately on a 250 ms
/// tick — so a scrobbler reading `player.read` would otherwise always see
/// zero. Mirroring the event keeps the player thread free of plugin concerns,
/// the same way the event bridge does.
#[derive(Debug, Default)]
pub struct PositionMirror {
    position_ms: Mutex<(u64, u64)>,
}

impl PositionMirror {
    pub fn record(&self, position_ms: u64, duration_ms: u64) {
        if let Ok(mut slot) = self.position_ms.lock() {
            *slot = (position_ms, duration_ms);
        }
    }

    fn get(&self) -> (u64, u64) {
        self.position_ms
            .lock()
            .map(|slot| *slot)
            .unwrap_or((0, 0))
    }
}

pub struct ShellPlayerBridge {
    player: PlayerHandle,
    position: Arc<PositionMirror>,
}

impl ShellPlayerBridge {
    pub fn new(player: PlayerHandle, position: Arc<PositionMirror>) -> Self {
        Self { player, position }
    }
}

impl PlayerBridge for ShellPlayerBridge {
    fn state(&self) -> BridgePlayerState {
        // A player that cannot report is reported as stopped rather than
        // failing the guest: `player.read` is an observation, and a plugin
        // should not have to distinguish "stopped" from "briefly unavailable".
        let Ok(state) = self.player.state() else {
            return BridgePlayerState::default();
        };
        let (position_ms, ticked_duration_ms) = self.position.get();
        BridgePlayerState {
            playing: state.status == Status::Playing,
            paused: state.status == Status::Paused,
            track_id: state.track.as_ref().map(|track| track.id),
            position_ms,
            // The tick's duration is authoritative once it has arrived; the
            // track's tag is the fallback before the first tick of a song.
            duration_ms: if ticked_duration_ms > 0 {
                ticked_duration_ms
            } else {
                state
                    .track
                    .as_ref()
                    .map(|track| track.duration_ms.max(0) as u64)
                    .unwrap_or(0)
            },
            volume: state.volume,
            muted: state.muted,
            shuffle: state.shuffle,
            repeat: Some(match state.repeat {
                RepeatMode::Off => BridgeRepeat::Off,
                RepeatMode::All => BridgeRepeat::All,
                RepeatMode::One => BridgeRepeat::One,
            }),
        }
    }

    fn control(&self, command: BridgePlayerCommand) -> Result<(), String> {
        let cmd = match command {
            BridgePlayerCommand::PlayPause => Cmd::TogglePause,
            BridgePlayerCommand::Stop => Cmd::Stop,
            BridgePlayerCommand::Next => Cmd::Next,
            BridgePlayerCommand::Previous => Cmd::Prev,
            BridgePlayerCommand::Seek { position_ms } => Cmd::Seek(position_ms),
            BridgePlayerCommand::SetVolume { volume } => Cmd::SetVolume(volume.clamp(0.0, 1.0)),
            BridgePlayerCommand::Enqueue { track_ids } => Cmd::QueueAdd(track_ids),
        };
        self.player.send(cmd)
    }
}

/// Re-emits plugin-initiated changes as the same Tauri events the UI already
/// listens to, so a playlist written through `crescendo:host/playlists`
/// refreshes the sidebar and reaches `on-playlist-changed` subscribers exactly
/// like one written through the normal command.
pub struct ShellHostNotifier {
    app: tauri::AppHandle,
}

impl ShellHostNotifier {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl HostNotifier for ShellHostNotifier {
    fn notify(&self, notification: HostNotification) {
        use tauri::Emitter;
        match notification {
            HostNotification::PlaylistChanged(id) => {
                let _ = self.app.emit("playlist:changed", id);
            }
        }
    }
}
