# Authoring plugins

Crescendo v1 plugins are data packages. They declare settings and a small set
of host-implemented contributions; they never run JavaScript, HTML, SQL, or
native code. Crescendo compiles accepted facet descriptors into its own SQL
query plans and renders controls itself.

A plugin is `kind: "declarative"` or `kind: "wasm"`. Declarative packages are
pure data. Wasm packages additionally carry one `module.wasm` component which
Crescendo executes in a sandbox (see *Wasm plugins* below).

## Source layout

A source directory may contain only:

```text
manifest.json
assets/                 # optional passive files
  ...
module.wasm             # Wasm-kind packages only
README.md               # optional source documentation; not packaged
LICENSE or LICENSE.txt  # optional source documentation; not packaged
```

Declarative v1 assets are limited to PNG, JPEG, WebP, GIF, BMP, text, Markdown,
and JSON. The same passive-asset rule applies to Wasm packages: `module.wasm`
is the only executable entry. SVG, CSS, HTML, JavaScript, files without an
extension, links, and special files are rejected. No v1 contribution serves
assets to the webview. Host features may load an asset only through a loaded
plugin ID plus a normalized asset ID; the resolver returns bounded bytes and a
host-selected media type, never an installed path.

Packages have a 32 MiB compressed limit, a 64 MiB extracted limit, a 256 KiB
manifest limit, and at most 512 entries. Paths must be UTF-8, relative,
forward-slash-separated, unique after normalization, and contained by one of
the roots above. Excessive compression ratios are rejected.

## Manifest

Point an editor at
[`plugin-sdk/manifest-v1.schema.json`](../../plugin-sdk/manifest-v1.schema.json).
The JSON Schema catches structural mistakes; the supported validator is
authoritative for semantic relationships and package rules. Unknown manifest
fields and unknown contributions are errors.

Every manifest supplies:

- `schema_version`: `1`.
- `id`: a lowercase reverse-domain ID with at least three segments.
- `name`, `description`, `authors`, `license`, and optional HTTP(S) `homepage`.
- semantic `version`, `api_version`, and `min_crescendo_version` values.
- `kind`: `declarative` or `wasm`.
- `permissions`: empty for declarative plugins; see *Capabilities* below.
- optional `contributes` declarations.

The canonical complete example is
[`plugins/dev.crescendo.primary-grouping/manifest.json`](../../plugins/dev.crescendo.primary-grouping/manifest.json).

## Contributions

Settings support four host-rendered types:

- `boolean` with a Boolean default;
- `string` with optional character-count minimum and maximum;
- `number` with optional minimum, maximum, and positive step;
- `select` with a non-empty set of value/label options.

Keys use lowercase letters, digits, and underscores. Defaults must satisfy the
declared constraints. Stored values are namespaced by plugin ID and setting
key. Setting writes apply immediately for a currently loaded plugin; installing,
updating, enabling, disabling, and uninstalling the loaded contribution set
take effect at the next start.

Declarative API 1.0 provides:

- `text.first_segment`: projects one canonical browsable tag to the text before
  the first declared delimiter, optionally trimming whitespace. Its
  `enabled_setting` must name a Boolean setting.
- a `library.filter` menu checkbox, conditionally visible for one active tag,
  whose `toggle_setting` action and `checked_setting` must name the same Boolean
  setting.

Only one enabled plugin may own a transform for a given target tag. The app
rejects conflicting enable requests rather than choosing an order implicitly.
Plugin data never supplies SQL; delimiters are quoted by trusted host code.

Wasm plugins additionally contribute `commands`: an id list (same key rules as
settings, at most 32). Only declared command ids are dispatchable through the
`plugin_invoke` application command, which is main-window only.

A menu control declares one of two actions:

```json
{ "type": "toggle_setting", "setting": "collapse_multi_tagged_titles" }
{ "type": "run_command", "command": "library_summary" }
```

A `toggle_setting` control names a `checked_setting`, and the host renders its
value as the item's tick. A `run_command` control is Wasm-only, has no
`checked_setting` — it shows no state — and may name only a command the same
manifest declares, so a menu item can never reach a command the plugin did not
publish. The action carries no arguments; the host dispatches it through the
same bounded path as `plugin_invoke`, outside the projection transition,
because running a command changes no projection.

## Wasm plugins

Wasm packages carry a `module.wasm` built against the versioned WIT world in
[`plugin-sdk/wit/`](../../plugin-sdk/wit) (runtime API `0.2.1`, matched by the
manifest's `api_version`). The component model interface is:

- `crescendo:host/host` — always available: an on-demand `settings` read of the
  plugin's live setting values and a bounded `log` for the plugin's
  diagnostics view.
- `crescendo:host/storage` — namespaced key/value storage (requires
  `storage`): 256-byte keys, 256 KiB values, 5 MiB quota.
- `crescendo:host/library` — windowed, id-ordered read-only track metadata
  (requires `library.read`; `path` fields are filled only with
  `library.paths`), pages of at most 2000 rows.
- `crescendo:host/player`, `crescendo:host/playlists`, `crescendo:host/net` —
  playback observation and bounded transport, playlist reads and writes, and
  HTTPS requests to approved domains, each behind its own capability.
- exported `crescendo:plugin/commands` — `init`, `invoke(command, input)`,
  `on-settings-changed`, and the four other event handlers listed below.

Import only the interfaces the plugin uses. Imports are matched by semver, so a
component keeps loading when a later host patch adds one. **Exports are the
opposite**: the world names every export a component must provide, so a missing
handler stops it from instantiating at all.

There is no WASI: no filesystem, environment, process, socket, clock, or
database access, and no per-track execution. Each plugin runs as one isolated
component on a dedicated runtime thread with a 64 MiB memory limit, per-call
fuel, and a wall-clock deadline. Repeated runtime failures mark the plugin
unhealthy for the session; repeated startup-init failures auto-disable it.

### Rust bindings

Rust plugins depend on the generated binding crate rather than running
`wit_bindgen::generate!` themselves, so every plugin is built from one copy of
the WIT and cannot drift from the host:

```toml
[dependencies]
crescendo-plugin-sdk = { path = "path/to/plugin-sdk/bindings/rust" }
```

```rust
use crescendo_plugin_sdk::{export_plugin, host, Guest, LogLevel};

struct Component;
export_plugin!(Component);

impl Guest for Component {
    fn init() -> Result<(), ()> {
        host::log(LogLevel::Info, "hello");
        Ok(())
    }
    // ... the remaining exports the world requires
}
```

The crate version tracks the runtime API it was generated from; keep it equal
to the `api_version` in the manifest.

### Build and package

Build the canonical example from
[`plugins/dev.crescendo.wasm-demo/`](../../plugins/dev.crescendo.wasm-demo):
it needs `rustup target add wasm32-unknown-unknown` and
`cargo install wasm-tools`. Running `./build.sh` there produces `module.wasm`
and stages `dist/` — manifest.json plus the built module, which is all a
package may contain — so the packer runs straight on it:

```bash
sh plugins/dev.crescendo.wasm-demo/build.sh
cargo run -p crescendo-core --bin crescendo-plugin -- pack plugins/dev.crescendo.wasm-demo/dist
```

Pack a staged directory, never the Rust source directory: `Cargo.toml`,
`src/`, and `target/` are not package content, and the validator refuses an
archive that carries them.

## Capabilities

Wasm manifests may request capabilities from the fixed v1 vocabulary:
`library.read`, `library.paths`, `player.read`, `player.control`,
`playlists.read`, `playlists.write`, `network.http`, `storage`. Rules:

- `library.paths` requires `library.read`.
- `network.http` requires a `network_domains` list (lowercase hostnames,
  at most 32).
- Declarative packages cannot request capabilities.

Grants are user-approved actions recorded against the plugin identity,
version, and package hash; any update voids the approval and must be approved
again before the update can load. Runtime API `0.2.1` provides the host
interface for every capability in the vocabulary — see the table below. The
compatibility check remains: a package naming a capability this build does not
know stays installed but incompatible, and cannot be enabled, until a later
Crescendo ships that capability.

## Validate and pack

From a Crescendo checkout:

```bash
# Validate source without creating an archive.
cargo run -p crescendo-core --bin crescendo-plugin -- validate path/to/source

# Create id-version.crescendo-plugin in the current directory.
cargo run -p crescendo-core --bin crescendo-plugin -- pack path/to/source

# Or choose an explicit output path.
cargo run -p crescendo-core --bin crescendo-plugin -- pack path/to/source dist/example.crescendo-plugin

# Revalidate a package received from elsewhere.
cargo run -p crescendo-core --bin crescendo-plugin -- validate dist/example.crescendo-plugin
```

Packing fixes ZIP timestamps and permissions, sorts file paths, omits directory
entries, and validates the completed archive through the runtime staging path.
Identical source bytes produce an identical archive and SHA-256. The output is
created without replacing an existing file.

## Capabilities in runtime API 0.2.1

Every permission in the v1 vocabulary now has a host interface. Each is denied
per call unless the user approved it for this exact package:

| Permission | Interface | Notes |
| --- | --- | --- |
| `library.read` | `crescendo:host/library` | Windowed pages, at most 2000 rows. |
| `library.paths` | same | Adds `path`; requires `library.read`. |
| `player.read` | `crescendo:host/player` | `get-state` only. |
| `player.control` | same | Transport, volume, and queue append. |
| `playlists.read` | `crescendo:host/playlists` | `all` and `tracks`. |
| `playlists.write` | same | Create, rename, add, remove. No delete in v1. |
| `network.http` | `crescendo:host/net` | HTTPS only, to approved domains. |
| `storage` | `crescendo:host/storage` | 5 MiB namespaced, byte-accounted. |

`network.http` also requires `network_domains` in the manifest. A domain
approves itself and its subdomains, never a bare suffix, and every request —
including each redirect hop — is re-checked against the list *and* against
where the host actually resolves, so an approved name pointing at loopback or a
private range is still refused.

### Events

A component exports five coalesced handlers besides `on-settings-changed`:
`on-ready`, `on-player-state-changed`, `on-track-changed`,
`on-library-changed`, and `on-playlist-changed`. They are notifications, not a
stream: the host drops duplicates under load and may drop an event entirely if
the runtime queue is full, so re-read what you need instead of accumulating
state from the callbacks.

Playlist mutations a plugin makes through `crescendo:host/playlists` refresh
the app's own playlist views: the host emits the same change notification a
mutation made through the UI does, so the sidebar updates and every plugin
with an `on-playlist-changed` handler hears it — including the plugin that
made the change, which should therefore not treat the callback as news.
