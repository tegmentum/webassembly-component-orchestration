#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "==> Building dynlink-endpoint-consumer (wasm32-wasip2, release)"
cargo build --release --target wasm32-wasip2

ARTIFACT="target/wasm32-wasip2/release/dynlink_endpoint_consumer.wasm"
echo "    component: $SCRIPT_DIR/$ARTIFACT"
