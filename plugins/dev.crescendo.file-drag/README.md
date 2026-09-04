# File Drag and Drop

This declarative plugin enables Crescendo's bounded outbound file-drag
surface. It adds no executable code and receives no file paths; Crescendo
resolves the user's visible selection and performs the native desktop drag.

One drag carries at most the first 2,000 available tracks.

Validate and create the installable package from the repository root:

```bash
cargo run -p crescendo-core --bin crescendo-plugin -- validate \
  plugins/dev.crescendo.file-drag
cargo run -p crescendo-core --bin crescendo-plugin -- pack \
  plugins/dev.crescendo.file-drag \
  plugins/dev.crescendo.file-drag-1.0.0.crescendo-plugin
```
