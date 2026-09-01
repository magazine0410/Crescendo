mod manager;
mod manifest;
mod package;
pub mod tooling;

pub use manager::{
    PluginAsset, PluginManager, PluginMenuControlInfo, PluginMutationResult, PluginSnapshot,
    StaleProjection, GROUPING_PLUGIN_ID, GROUPING_PLUGIN_SETTING,
};
pub use manifest::{
    PluginContributions, PluginFacetTransform, PluginKind, PluginManifest, PluginMenuAction,
    PluginMenuControl, PluginMenuSlot, PluginMenuVisibility, PluginSelectOption,
    PluginSettingDeclaration, PluginSettingValue,
};

/// Declarative plugin API understood by this build. A manifest may target an
/// older compatible 1.x API, but never a newer one or another major version.
pub const PLUGIN_API_VERSION: &str = "1.0.0";
