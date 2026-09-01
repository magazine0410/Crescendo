# Authoring declarative plugins

Crescendo v1 plugins are data packages. They declare settings and a small set
of host-implemented contributions; they do not run JavaScript, HTML, SQL, Wasm,
or native code. Crescendo compiles accepted facet descriptors into its own SQL
query plans, renders controls itself, and validates setting values before they
are stored.

## Source layout

A source directory may contain only:

```text
manifest.json
assets/                 # optional passive files
  ...
module.wasm             # Wasm-kind packages only; unsupported until Stage 5
README.md               # optional source documentation; not packaged
LICENSE or LICENSE.txt  # optional source documentation; not packaged
```

Declarative v1 assets are limited to PNG, JPEG, WebP, GIF, BMP, text, Markdown,
and JSON. SVG, CSS, HTML, JavaScript, Wasm, files without an extension, links,
and special files are rejected. No v1 contribution serves assets to the
webview. Host features may load an asset only through a loaded plugin ID plus a
normalized asset ID; the resolver returns bounded bytes and a host-selected
media type, never an installed path.

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
- `kind`: `declarative` for v1.
- `permissions`: empty for declarative plugins.
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
