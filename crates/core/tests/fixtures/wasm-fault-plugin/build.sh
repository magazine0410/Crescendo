#!/bin/sh
# Rebuild the committed fixture ../wasm-fault-module.wasm after changing this
# plugin. Requires the wasm32-unknown-unknown target and wasm-tools.
# Stripped, so the file does not vary with the checkout's path.
set -eu

cd "$(dirname "$0")"

cargo build --release --target wasm32-unknown-unknown

wasm-tools component new \
    target/wasm32-unknown-unknown/release/crescendo_wasm_fault.wasm \
    -o target/module.wasm
wasm-tools strip --all target/module.wasm -o ../wasm-fault-module.wasm
wasm-tools component wit ../wasm-fault-module.wasm >/dev/null

echo "built ../wasm-fault-module.wasm"
