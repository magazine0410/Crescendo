mod bridge;
mod manager;
mod manifest;
mod net;
mod package;
pub mod permissions;
pub mod tooling;
pub mod wasm;

pub use bridge::{
    BridgePlayerCommand, BridgePlayerState, BridgeRepeat, HostNotification, HostNotifier,
    HostNotifierSlot, PlayerBridge, PlayerBridgeSlot,
};
pub use manager::{
    PluginAsset, PluginInfo, PluginManager, PluginMenuControlInfo, PluginMutationResult,
    PluginSnapshot, StaleProjection, GROUPING_PLUGIN_ID, GROUPING_PLUGIN_SETTING,
};
pub use manifest::{
    PluginCommandDeclaration, PluginContributions, PluginFacetTransform, PluginKind,
    PluginManifest, PluginMenuAction, PluginMenuControl, PluginMenuSlot, PluginMenuVisibility,
    PluginSelectOption, PluginSettingDeclaration, PluginSettingValue,
};
pub use permissions::{
    parse_network_domains, parse_permissions, parse_stored_network_domains, PluginGrant,
    PluginPermission,
};
pub use wasm::{WasmHandle, WasmPluginSpec};

/// Declarative plugin API understood by this build. A manifest may target an
/// older compatible 1.x API, but never a newer one or another major version.
pub const PLUGIN_API_VERSION: &str = "1.0.0";

/// Wasm runtime API provided by this build, versioned independently from the
/// declarative manifest API. 0.x marks the Stage 5 experimental runtime: the
/// host provides the `crescendo:host` WIT interfaces for settings, bounded
/// storage, and windowed library reads.
pub const WASM_API_VERSION: &str = "0.2.0";
