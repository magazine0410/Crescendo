# Declarative v1 release checklist

Run these gates from the repository root before cutting a release:

```bash
cargo test -p crescendo-core
cargo test --release -p crescendo-core \
  --test plugin_projection_performance_test -- --ignored --nocapture
cargo check -p crescendo
npm run check
cargo run -p crescendo-core --bin crescendo-plugin -- validate \
  plugins/dev.crescendo.primary-grouping
```

The SDK workflow packs the canonical Primary Grouping source, validates the
finished package, preserves it as a CI artifact, and attaches it to a published
GitHub release. Install that release asset in a clean Crescendo profile and
confirm it starts disabled, requires one restart to enter the loaded set, and
then toggles grouping projection live.

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

