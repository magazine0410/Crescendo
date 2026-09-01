//! Host services the plugin runtime needs but `crescendo-core` cannot own.
//!
//! The player lives in `src-tauri` (it owns a GStreamer thread), so core
//! declares the shape it needs and the shell installs an implementation. The
//! slot exists from the moment `PluginManager` starts, because the manager is
//! constructed before the player thread; calls that arrive before the shell
//! fills it are answered `Unavailable` rather than blocking or panicking.
//!
//! Nothing here takes a plugin id: capability checks happen in the runtime,
//! against the identity bound to the calling store.

use std::sync::{Arc, RwLock};

/// Repeat mode, mirroring `crate::playqueue::RepeatMode` without depending on
/// the player's own types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeRepeat {
    Off,
    All,
    One,
}

/// A snapshot of playback for `player.read`.
#[derive(Debug, Clone, Default)]
pub struct BridgePlayerState {
    pub playing: bool,
    pub paused: bool,
    pub track_id: Option<i64>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f64,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Option<BridgeRepeat>,
}

/// One bounded playback action for `player.control`. The variants are exactly
/// the operations the WIT interface exposes; there is no passthrough.
#[derive(Debug, Clone)]
pub enum BridgePlayerCommand {
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek { position_ms: u64 },
    SetVolume { volume: f64 },
    Enqueue { track_ids: Vec<i64> },
}

/// Something that changed and the rest of the app must hear about.
///
/// A plugin writing through `crescendo:host/playlists` bypasses the Tauri
/// command that normally emits `playlist:changed`, so without this the sidebar
/// and other plugins would silently go stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostNotification {
    /// A playlist was created, renamed, or had its tracks change.
    PlaylistChanged(Option<i64>),
}

/// Implemented by the shell so core can announce plugin-initiated changes.
pub trait HostNotifier: Send + Sync {
    fn notify(&self, notification: HostNotification);
}

/// Late-filled like [`PlayerBridgeSlot`]; announcements before the shell wires
/// itself up are simply dropped.
#[derive(Default)]
pub struct HostNotifierSlot {
    notifier: RwLock<Option<Arc<dyn HostNotifier>>>,
}

impl std::fmt::Debug for HostNotifierSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HostNotifierSlot").finish()
    }
}

impl HostNotifierSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn install(&self, notifier: Arc<dyn HostNotifier>) {
        if let Ok(mut slot) = self.notifier.write() {
            *slot = Some(notifier);
        }
    }

    pub fn announce(&self, notification: HostNotification) {
        let notifier = self.notifier.read().ok().and_then(|slot| slot.clone());
        if let Some(notifier) = notifier {
            notifier.notify(notification);
        }
    }
}

/// Implemented by the shell over its real player handle.
pub trait PlayerBridge: Send + Sync {
    fn state(&self) -> BridgePlayerState;
    fn control(&self, command: BridgePlayerCommand) -> Result<(), String>;
}

/// A late-filled slot so `PluginManager::start` can run before the player
/// thread exists.
#[derive(Default)]
pub struct PlayerBridgeSlot {
    bridge: RwLock<Option<Arc<dyn PlayerBridge>>>,
}

impl std::fmt::Debug for PlayerBridgeSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlayerBridgeSlot")
            .field("installed", &self.get().is_some())
            .finish()
    }
}

impl PlayerBridgeSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install the shell's player. Called once, after the player thread spawns.
    pub fn install(&self, bridge: Arc<dyn PlayerBridge>) {
        if let Ok(mut slot) = self.bridge.write() {
            *slot = Some(bridge);
        }
    }

    pub fn get(&self) -> Option<Arc<dyn PlayerBridge>> {
        self.bridge.read().ok().and_then(|slot| slot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    impl PlayerBridge for Fake {
        fn state(&self) -> BridgePlayerState {
            BridgePlayerState {
                playing: true,
                volume: 0.5,
                ..Default::default()
            }
        }

        fn control(&self, _command: BridgePlayerCommand) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn an_empty_slot_reads_as_unavailable_until_the_shell_installs_a_player() {
        let slot = PlayerBridgeSlot::new();
        assert!(slot.get().is_none());
        slot.install(Arc::new(Fake));
        let bridge = slot.get().expect("installed");
        assert!(bridge.state().playing);
        assert!(bridge.control(BridgePlayerCommand::Stop).is_ok());
    }
}
