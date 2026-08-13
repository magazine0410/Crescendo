# projectM WebAssembly integration

Crescendo uses libprojectM 4.1.5 through the WebAssembly/WebGL2 path documented by projectM in `EMSCRIPTEN.md`:

- SDL2 creates the WebGL2/OpenGL ES 3 context on the Visualizer canvas.
- `OES_texture_float` is enabled before `projectm_create()`.
- Crescendo supplies its own interleaved stereo PCM through the public C API.
- The audible GStreamer stream is unchanged; only a leaky visualization side branch is resampled.

The checked-in `static/projectm/projectm.{js,wasm,data}` files were built with Emscripten 6.0.6. `projectm.data` contains the official MilkDrop base texture pack plus the textures shipped with the En D collection. The 40 `.milk` files currently present in the upstream En D repository are copied separately under `static/projectm/presets/en-d/`.

## Rebuilding

Activate an Emscripten SDK, then run:

```bash
third_party/projectm/build-wasm.sh
```

The script uses the corresponding-source archives under `third_party/projectm/source/`, builds projectM with its official CMake Emscripten target, and replaces the three generated runtime files. Run `node third_party/projectm/prepare-presets.mjs <extracted-en-d-dir>` only when updating the preset pack.

Upstream revisions:

- projectM 4.1.5: `976c70be1074b0fe9ee1768ecd380a80e42085b7`
- projectM Eval: `612afc49a4ee05dcfd68548a5c9bf1aaa031d17d`
- En D presets: `fff71ea81223109f3558351667eef851f2781c96`
- MilkDrop texture pack: `b6e461010ffbb78939d5b3c2248bb98af77411c3`

libprojectM is LGPL-2.1-or-later; its license is included as `LICENSE-projectM.txt`. The En D repository's preset notice is included as `LICENSE-En-D.md`. The texture pack has no separate license file upstream; its README and source URL are retained in `NOTICE-textures.md`.
