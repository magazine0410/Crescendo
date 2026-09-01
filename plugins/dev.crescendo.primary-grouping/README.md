# Primary Grouping

This is Crescendo's canonical declarative-plugin example. It contributes one
host-implemented `text.first_segment` transform for the `grouping` tag and one
host-rendered checkbox in the Library filter menu. No plugin code or SQL runs.

Validate and create the release package from the repository root:

```bash
cargo run -p crescendo-core --bin crescendo-plugin -- validate plugins/dev.crescendo.primary-grouping
cargo run -p crescendo-core --bin crescendo-plugin -- pack \
  plugins/dev.crescendo.primary-grouping \
  dev.crescendo.primary-grouping-1.0.0.crescendo-plugin
```

The packer produces reproducible ZIP bytes and prints the SHA-256. See the
[`plugin-sdk`](../../plugin-sdk/README.md) and
[authoring guide](../../docs/plugins/authoring.md) for the complete contract.
