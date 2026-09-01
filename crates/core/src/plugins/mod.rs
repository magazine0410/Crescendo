mod manager;
mod manifest;
mod package;

pub use manager::{PluginManager, PluginMutationResult, PluginSnapshot};
pub use manifest::{
    PluginContributions, PluginKind, PluginManifest, PluginSelectOption, PluginSettingDeclaration,
    PluginSettingValue,
};

/// Declarative plugin API understood by this build. A manifest may target an
/// older compatible 1.x API, but never a newer one or another major version.
pub const PLUGIN_API_VERSION: &str = "1.0.0";
