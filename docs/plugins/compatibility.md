# Plugin compatibility and versioning

Crescendo uses three independent versions in `manifest.json`.

`schema_version` identifies the manifest shape. This release accepts exactly
version 1. A different value, an unknown field, or an unknown contribution is a
validation error; Crescendo leaves the package disabled and reports the error.

`api_version` identifies contribution semantics. For declarative packages the
host currently implements `1.0.0`. A plugin is compatible when its API major
matches the host and its version is not newer than the host API. For example,
an API `1.0.0` package is accepted by host `1.1.0`, while `1.1.0` is not
accepted by host `1.0.0`, and API `2.x` requires an explicit new compatibility
contract.

For `kind: "wasm"` packages, `api_version` is checked the same way against the
sandbox runtime API (`0.2.1` in this release), which versions the whole runtime
contract — the WIT world *and* the host-side contribution vocabulary a Wasm
package may declare — independently of the declarative manifest API.

Every capability in the v1 vocabulary has a host interface, so no permission is
refused for being unimplemented. The check remains as the mechanism: a package
naming a capability a future build has not shipped stays installed and cannot
be enabled, rather than loading against a host that would silently ignore it.

`min_crescendo_version` is the oldest application version the author supports.
It must not be newer than the running Crescendo version. The package can remain
installed when incompatible, but it cannot be enabled.

The package `version` is the plugin's own semantic version. Updating an
installed plugin requires the same plugin ID and a strictly higher package
version. Use patch releases for compatible fixes, minor releases for compatible
new contributions or settings, and a major release when plugin behavior or
stored-data expectations change incompatibly.

Package metadata in `manifest.json` is authoritative. Crescendo stores the
desired enabled state, pending lifecycle journal, package SHA-256, and
namespaced setting values separately. Uninstalling may keep settings for a
future reinstall or remove them at the user's choice.

## API evolution rules

The rules below are what `compatibility_error` implements, and
`api_evolution_rules_decide_which_packages_this_host_accepts` in
`crates/core/src/plugins/manager.rs` holds them to it.

- **A package never loads against a host older than itself.** `api_version`
  must be less than or equal to the host's version for its API.
- **A different major is always incompatible**, in either direction.
- **While the runtime API is below `1.0.0`, a differing minor is breaking.**
  The exported `commands` interface is part of the world, so adding an export
  stops an older component from instantiating: `0.2.0` added the five event
  handlers, and a `0.1.0` component is reported incompatible rather than loaded
  and trapped.
- **A patch release is a compatible addition.** `0.2.1` added the
  `run_command` menu action without touching the world, so components built
  against `0.2.0` keep loading. Host *imports* are matched by semver — a
  component asking for `crescendo:host/library@0.2.0` binds to this host's
  `@0.2.1` — which is why adding a host interface is a patch and adding an
  export is not. `crates/core/tests/fixtures/wasm-legacy-0.2.0-module.wasm` is
  a frozen 0.2.0 component kept precisely to keep proving this; never rebuild
  it.
- **The two API versions move independently.** A Wasm package naming the
  declarative API version, or the reverse, is incompatible.
- **The manifest schema is a third, separate version.** `schema_version` is
  still 1 and changes only when the manifest *shape* changes.

Author guidance: raise `api_version` only when the plugin actually needs
something the newer API added. A package that declares the newest version it
was built against is refused by every older Crescendo, including the one the
user is running.

The declarative contribution set is immutable for a running process. First
install is immediately listed but disabled; install, update, enable, disable,
and uninstall changes are reflected after the required restart. Values for
declared settings use a guarded write-then-publish path and apply live. Views
and newly created play contexts carry a projection revision so one response
cannot combine old and new projection semantics.

