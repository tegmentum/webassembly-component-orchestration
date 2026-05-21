#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "Building compose-orchestrator-wasm for wasm32-wasip2..."
cargo build --release --target wasm32-wasip2
ARTIFACT="$(git rev-parse --show-toplevel)/target/wasm32-wasip2/release/compose_orchestrator_wasm.wasm"
echo "Component artifact: $ARTIFACT"

if command -v wasm-tools &> /dev/null; then
    echo
    echo "World declared by the component:"
    wasm-tools component wit "$ARTIFACT"
fi
