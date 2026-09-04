# Plugin release checklist

Run these gates from the repository root before cutting a release:

```bash
cargo fmt --all --check
cargo test -p crescendo-core
cargo test --release -p crescendo-core \
  --test plugin_projection_performance_test -- --ignored --nocapture
cargo check -p crescendo
npm run check
cargo run -p crescendo-core --bin crescendo-plugin -- validate \
  plugins/dev.crescendo.primary-grouping
cargo run -p crescendo-core --bin crescendo-plugin -- validate \
  plugins/dev.crescendo.file-drag
sh plugins/dev.crescendo.wasm-demo/build.sh
cargo run -p crescendo-core --bin crescendo-plugin -- validate \
  plugins/dev.crescendo.wasm-demo/dist
```

The last two need `rustup target add wasm32-unknown-unknown` and
`cargo install wasm-tools`. If the rebuilt `module.wasm` differs from
`crates/core/tests/fixtures/wasm-demo-module.wasm`, copy it over: the fixture
is what the runtime tests actually run.

Compare the two with `wasm-tools strip --all` applied to both, not with a raw
`cmp`. Rust hashes each crate's absolute path into its mangled symbols, so the
name section differs between any two checkout locations while the code is
identical. That is what the CI provenance job compares, together with the WIT
world as text.

The SDK workflow packs the canonical declarative sources, validates the
finished packages, preserves them as CI artifacts, and attaches them to a
published GitHub release. Install each release asset in a clean Crescendo
profile and confirm it starts disabled and requires one restart to enter the
loaded set; Primary Grouping then toggles projection live, while File Drag and
Drop alone enables outbound dragging.

## Automated invariants

- Source and package tooling tests cover the shipped schema, unsupported
  manifest shapes, deterministic bytes and SHA-256, runtime staging validation,
  and active declarative content rejection.
- Package and manager tests cover size/path/link/decompression limits,
  malformed-package startup isolation, safe mode, disabled first install,
  deferred contribution-set changes, live setting publication, conflicting
  facet ownership, interrupted update recovery/rollback, uninstall data
  semantics, and migration completion.
- The Tauri build checks exact parity between `generate_handler!` and
  `AppManifest::commands`, requires every plugin permission in the main
  capability, and rejects one in the About or shared cover-window capability.
- The 150k-track release test runs exact and transformed facets through SQLite,
  compares query time with a bounded allowance, and checks counts, windowed
  rows, albums, group boundaries, and playback IDs. No transform callback runs
  once per row in application code and no library command returns a complete
  track list over IPC.
- Projection guards, frontend request generations, the transition transaction,
  and stale-command typing prevent mixed-generation view commits. Play actions
  are disabled during publication; a stale play request refreshes once and
  retries by stable track ID, then reports that the view changed.
- The player thread owns concrete context IDs and the ad-hoc queue contains
  concrete track IDs. A projection setting write does not send a player command,
  so active playback and queued IDs remain unchanged.
- The Wasm runtime tests run the real engine against the committed demo
  component: capability denial per interface, grant pinning to version and
  hash, storage namespacing and quota, host validation of plugin-supplied
  playlist and queue ids, fuel/epoch/memory/table bounds, isolated failure and
  auto-disable, and the network guard's single resolution.
- Compatibility is proven both ways: the version matrix in
  `api_evolution_rules_decide_which_packages_this_host_accepts`, and a frozen
  0.2.0 component that must keep loading and reaching the host on today's
  runtime.
- The SDK workflow checks that the runtime API version, both WIT package
  declarations, and the Rust binding crate agree, and builds the binding crate
  for `wasm32-unknown-unknown`.
- The formatting workflow runs `cargo fmt --check` over the workspace *and*
  over the two crates outside it — the Rust bindings and the demo plugin —
  which a root `cargo fmt` does not reach.

## Manual release checks

- Upgrade a pre-plugin database once with collapse enabled: accept the notice,
  restart, verify the behavior, and confirm the notice never returns.
- Repeat with exact grouping enabled and with a fresh profile; neither should
  install a plugin or show an offer.
- Interrupt a staged update and a loaded-plugin uninstall at process boundaries,
  then confirm the journal resumes or restores the old package without loss.
- Leave a startup marker, confirm one safe-mode run with no contributions, and
  confirm the following clean launch restores the desired loaded set.
- Start once with `CRESCENDO_DISABLE_PLUGINS=1` and confirm the diagnostic reason
  is visible while the library and player remain usable.
- Install the exact `.crescendo-plugin` file attached to the GitHub release and
  compare its printed/validated SHA-256 with the release artifact.
- With no file-drag provider loaded, confirm rows have their normal cursor and
  `plugin_file_drag_start` is refused; after enabling the official provider and
  restarting, confirm quick and 2,000-track-limited drops work.
- Install the packed Wasm demo, approve its capabilities, restart, and confirm:
  its commands run from the Plugins page; `spin` fails on the deadline without
  disturbing playback; the player, playlist, and network commands answer
  "denied" because it never requested those capabilities; and, with the CUSTOM
  filter set to `composer`, its menu action appears and runs.
- Confirm the release carries `crescendo-plugin-sdk.tar.gz` and that a plugin
  built from that copy of `plugin-sdk/` loads in the released build.
