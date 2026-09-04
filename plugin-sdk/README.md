# Crescendo plugin SDK

This directory is the versioned authoring contract for Crescendo's plugin
system. The current contracts are manifest schema version 1, declarative plugin
API `1.1.0`, and the sandboxed Wasm runtime API `0.2.1`. Each release attaches
a copy of this directory to its GitHub release, so a plugin can be built
against the exact contract a given Crescendo implements.

- [`manifest-v1.schema.json`](manifest-v1.schema.json) is the editor-facing
  JSON Schema.
- [`wit/`](wit/) is the versioned WIT world every Wasm plugin component
  imports and exports (see `wit/crescendo-plugin.wit`).
- [`bindings/rust/`](bindings/rust/) is the generated Rust binding crate a
  Rust plugin depends on instead of invoking `wit_bindgen::generate!` itself.
  Its version tracks the runtime API. Rust is the toolchain Crescendo ships
  bindings for; any language with a Component Model generator can build against
  `wit/` directly, but only the Rust path is tested here.
- [`../plugins/dev.crescendo.primary-grouping/`](../plugins/dev.crescendo.primary-grouping/)
  is the canonical declarative source example.
- [`../plugins/dev.crescendo.file-drag/`](../plugins/dev.crescendo.file-drag/)
  is the installable outbound file-drag contribution.
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
- [`../docs/plugins/security-audit.md`](../docs/plugins/security-audit.md) is
  the capability, replacement, exhaustion, and confused-deputy review, with the
  test that keeps each boundary honest.

Run the supported validator and deterministic packer from the repository root:

```bash
cargo run -p crescendo-core --bin crescendo-plugin -- validate plugins/dev.crescendo.primary-grouping
cargo run -p crescendo-core --bin crescendo-plugin -- pack plugins/dev.crescendo.primary-grouping
```

Pack a *staged* directory: a package may contain only `manifest.json`,
`assets/`, and `module.wasm`, so a Wasm plugin's Rust source directory is not
packable. `plugins/dev.crescendo.wasm-demo/build.sh` stages `dist/` for exactly
this reason.

`pack` refuses to overwrite an existing archive and prints the archive's
SHA-256. `validate` accepts either an unpacked source directory or a finished
`.crescendo-plugin` package; package validation also prints its SHA-256. The
command uses the same manifest, archive, path, size, and passive-content checks
as Crescendo's installer.

## Distribution

Distribution is local and manual by design. A plugin is a file the user chose
and installed; Crescendo has no marketplace, no signing authority, no
dependency resolution, and no auto-update protocol, and the installer says
plainly that a package is locally supplied code. Those are separate projects,
not omissions — see
[`../docs/plugins/security-audit.md`](../docs/plugins/security-audit.md) for
what that does and does not defend against.

Official packages are attached to the GitHub release that implements their
contract, together with `crescendo-plugin-sdk.tar.gz`, a copy of this
directory. Check a downloaded package with `validate`, which prints the same
SHA-256 the release notes carry.
