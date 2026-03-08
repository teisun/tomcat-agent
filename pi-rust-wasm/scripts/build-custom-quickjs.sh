#!/usr/bin/env bash
# Build custom wasmedge_quickjs.wasm with __pi_host_call support.
# Requires: Rust stable toolchain with wasm32-wasip1 target.
# Usage: ./scripts/build-custom-quickjs.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PI_RUST_WASM_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
QUICKJS_DIR="$(cd "$PI_RUST_WASM_DIR/../wasmedge-quickjs" && pwd)"

if [ ! -d "$QUICKJS_DIR" ]; then
  echo "Error: wasmedge-quickjs not found at $QUICKJS_DIR"
  echo "Clone it first: git clone git@github.com:second-state/wasmedge-quickjs.git $QUICKJS_DIR"
  exit 1
fi

echo "Building custom wasmedge_quickjs.wasm from $QUICKJS_DIR ..."
cd "$QUICKJS_DIR"
cargo +stable build --release --no-default-features --bin wasmedge_quickjs

WASM_SRC="$QUICKJS_DIR/target/wasm32-wasip1/release/wasmedge_quickjs.wasm"
WASM_DST="$PI_RUST_WASM_DIR/assets/wasm/wasmedge_quickjs.wasm"

if [ ! -f "$WASM_SRC" ]; then
  echo "Error: build succeeded but wasm not found at $WASM_SRC"
  exit 1
fi

cp "$WASM_SRC" "$WASM_DST"
echo "Copied to $WASM_DST ($(du -h "$WASM_DST" | cut -f1))"
echo "Done. Verify: strings $WASM_DST | grep __pi_host_call"
