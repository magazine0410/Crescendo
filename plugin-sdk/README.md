# Crescendo plugin SDK

This directory is the versioned authoring contract for Crescendo's plugin
system. The current contracts are manifest schema version 1, declarative plugin
API `1.0.0`, and the sandboxed Wasm runtime API `0.2.0`.

- [`manifest-v1.schema.json`](manifest-v1.schema.json) is the editor-facing
  JSON Schema.
- [`wit/`](wit/) is the versioned WIT world every Wasm plugin component
  imports and exports (see `wit/crescendo-plugin.wit`).
- [`../plugins/dev.crescendo.primary-grouping/`](../plugins/dev.crescendo.primary-grouping/)
  is the canonical declarative source example.
- [`../plugins/dev.crescendo.wasm-demo/`](../plugins/dev.crescendo.wasm-demo/)
  is the canonical sandboxed Wasm source example.
- [`../docs/plugins/authoring.md`](../docs/plugins/authoring.md) explains the
  supported contribution vocabulary, capabilities, and package command.
- [`../docs/plugins/compatibility.md`](../docs/plugins/compatibility.md)
  defines version compatibility.
- [`../docs/plugins/operations.md`](../docs/plugins/operations.md) covers
  installation, safe mode, and recovery.
- [`../docs/plugins/release-checklist.md`](../docs/plugins/release-checklist.md)
  records the automated and manual release gates.

Run the supported validator and deterministic packer from the repository root:

```bash
cargo run -p crescendo-core --bin crescendo-plugin -- validate plugins/dev.crescendo.primary-grouping
cargo run -p crescendo-core --bin crescendo-plugin -- pack plugins/dev.crescendo.primary-grouping
```

`pack` refuses to overwrite an existing archive and prints the archive's
SHA-256. `validate` accepts either an unpacked source directory or a finished
`.crescendo-plugin` package; package validation also prints its SHA-256. The
command uses the same manifest, archive, path, size, and passive-content checks
as Crescendo's installer.
