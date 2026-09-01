#!/bin/sh
# Build the demo plugin's sandboxed component from this source directory.
#
# Requires: rustup target add wasm32-unknown-unknown, wasm-tools (cargo
# install wasm-tools), and a Rust toolchain with the `wasm32-unknown-unknown`
# std installed. The output module.wasm is what the .crescendo-plugin package
# carries. Its committed copy is the runtime test fixture at
# crates/core/tests/fixtures/wasm-demo-module.wasm — refresh that file whenever
# this source changes, or wasm_runtime_test exercises a stale component.
set -eu

cd "$(dirname "$0")"

cargo build --release --target wasm32-unknown-unknown

wasm-tools component new \
    target/wasm32-unknown-unknown/release/crescendo_wasm_demo.wasm \
    -o module.wasm

wasm-tools component wit module.wasm >/dev/null
echo "built module.wasm"
