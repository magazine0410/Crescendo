mod bridge;
pub mod catalog;
pub mod export;
mod manager;
mod manifest;
pub(crate) mod net;
mod package;
pub mod permissions;
pub mod tooling;
pub mod wasm;

pub use bridge::{
    BridgePlayerCommand, BridgePlayerState, BridgeRepeat, HostNotification, HostNotifier,
    HostNotifierSlot, PlayerBridge, PlayerBridgeSlot,
};
pub use manager::{
    Enablement, PlaylistExporterInfo, PluginAsset, PluginInfo, PluginManager,
    PluginMenuControlInfo, PluginMutationResult, PluginSnapshot, StaleProjection,
};
pub use manifest::{
    PluginCommandDeclaration, PluginContributions, PluginFacetTransform, PluginKind,
    PluginManifest, PluginMenuAction, PluginMenuControl, PluginMenuSlot, PluginMenuVisibility,
    PluginPlaylistExporter, PluginSelectOption, PluginSettingDeclaration, PluginSettingValue,
};
pub use permissions::{
    parse_network_domains, parse_permissions, parse_stored_network_domains, PluginGrant,
    PluginPermission,
};
pub use wasm::{WasmHandle, WasmPluginSpec};

/// Declarative plugin API understood by this build. A manifest may target an
/// older compatible 1.x API, but never a newer one or another major version.
pub const PLUGIN_API_VERSION: &str = "1.1.0";

/// Wasm runtime API provided by this build, versioned independently from the
/// declarative manifest API. It covers the whole runtime contract: the
/// `crescendo:host` WIT world *and* the host-side contribution vocabulary a
/// Wasm package may declare.
///
/// While this is below 1.0.0 a differing **minor** is breaking, because the
/// exported `commands` interface is part of the world: a component missing a
/// newly added export cannot instantiate. A **patch** is a compatible
/// addition — 0.2.1 added the `run_command` menu action and 0.2.2 the
/// `playlist_exporters` contribution, neither touching the world's functions,
/// so components built against 0.2.0 keep loading. Wasmtime resolves a
/// component's `@0.2.0` imports against this host's `@0.2.2` interfaces by
/// semver, and `tests/wasm_runtime_test.rs` holds a 0.2.0-built component to
/// keep proving it.
pub const WASM_API_VERSION: &str = "0.2.2";
