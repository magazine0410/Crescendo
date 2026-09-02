//! Generated Rust bindings for the Crescendo sandboxed plugin world.
//!
//! A plugin depends on this crate instead of running `wit_bindgen::generate!`
//! itself, so every Rust plugin is built from one copy of the WIT in
//! `plugin-sdk/wit/` and cannot drift from the host by editing a macro
//! invocation. Write a component like this:
//!
//! ```ignore
//! use crescendo_plugin_sdk::{export_plugin, host, Guest, LogLevel};
//!
//! struct Component;
//! export_plugin!(Component);
//!
//! impl Guest for Component {
//!     fn init() -> Result<(), ()> {
//!         host::log(LogLevel::Info, "hello");
//!         Ok(())
//!     }
//!     // ... the remaining exports the world requires
//! }
//! ```
//!
//! Every export in `Guest` is mandatory: the world is the contract, and a
//! component missing one of them cannot instantiate. The host imports are
//! optional — import only the interfaces the plugin uses, and a component
//! keeps loading when a later runtime adds more.
//!
//! Calls into `library`, `storage`, `player`, `playlists`, and `net` are
//! capability-gated. Each returns its own error variant when the user has not
//! approved the matching manifest permission for this exact package; treat a
//! denial as an ordinary outcome, not a failure to report as a crash.

wit_bindgen::generate!({
    path: "../../wit",
    world: "crescendo-plugin",
    generate_all,
    pub_export_macro: true,
    export_macro_name: "export_plugin",
    default_bindings_module: "crescendo_plugin_sdk",
});

pub use crate::crescendo::host::host;
pub use crate::crescendo::host::library;
pub use crate::crescendo::host::net;
pub use crate::crescendo::host::player;
pub use crate::crescendo::host::playlists;
pub use crate::crescendo::host::storage;

// Only the names that are unambiguous across interfaces are lifted to the
// crate root. Records with interface-local names — `library::Page`,
// `player::State`, `playlists::Info` — stay behind their module, where they
// read correctly.
pub use crate::crescendo::host::host::{LogLevel, Setting};
pub use crate::crescendo::host::library::LibraryError;
pub use crate::crescendo::host::net::NetError;
pub use crate::crescendo::host::player::PlayerError;
pub use crate::crescendo::host::playlists::PlaylistError;
pub use crate::crescendo::host::storage::StorageError;

/// The interface every plugin component exports. Implement it on one type and
/// hand that type to [`export_plugin!`].
pub use crate::exports::crescendo::plugin::commands::Guest;
