#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
echo "==> Building hello-component (wasm32-wasip2, release)"
cargo build --release --target wasm32-wasip2
echo "    component: $SCRIPT_DIR/target/wasm32-wasip2/release/hello-component.wasm"
