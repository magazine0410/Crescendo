#!/bin/sh
# Build the demo plugin's sandboxed component and stage a packable directory.
#
# Requires: rustup target add wasm32-unknown-unknown, wasm-tools (cargo
# install wasm-tools), and a Rust toolchain with the `wasm32-unknown-unknown`
# std installed.
#
# Outputs:
#   module.wasm  the built component. Its committed copy is the runtime test
#                fixture at crates/core/tests/fixtures/wasm-demo-module.wasm —
#                refresh that file whenever this source changes, or
#                wasm_runtime_test exercises a stale component. The two are
#                compared after `wasm-tools strip --all`: Rust hashes each
#                crate's absolute path into its symbol names, so two checkouts
#                in different directories produce identical code under
#                different names.
#   dist/        manifest.json + module.wasm and nothing else, which is what a
#                .crescendo-plugin package may contain. Pack it with:
#                  cargo run -p crescendo-core --bin crescendo-plugin -- \
#                      pack plugins/dev.crescendo.wasm-demo/dist
set -eu

cd "$(dirname "$0")"

cargo build --release --target wasm32-unknown-unknown

wasm-tools component new \
    target/wasm32-unknown-unknown/release/crescendo_wasm_demo.wasm \
    -o module.wasm

wasm-tools component wit module.wasm >/dev/null

rm -rf dist
mkdir dist
cp manifest.json module.wasm dist/

echo "built module.wasm and staged dist/"
