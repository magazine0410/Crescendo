# Crescendo declarative plugin SDK

This directory is the versioned authoring contract for Crescendo's declarative
plugin API. The current contract is manifest schema version 1 and plugin API
`1.0.0`.

- [`manifest-v1.schema.json`](manifest-v1.schema.json) is the editor-facing
  JSON Schema.
- [`../plugins/dev.crescendo.primary-grouping/`](../plugins/dev.crescendo.primary-grouping/)
  is the canonical source example.
- [`../docs/plugins/authoring.md`](../docs/plugins/authoring.md) explains the
  supported contribution vocabulary and package command.
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
