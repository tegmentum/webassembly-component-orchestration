#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)
WAT="$REPO_ROOT/hosts/wasmtime/resources/placeholder.component.wat"
OUT_DIR="$REPO_ROOT/target/compose"
OUT_FILE="$OUT_DIR/placeholder.component.wasm"

if ! command -v wasm-tools >/dev/null 2>&1; then
  echo "error: wasm-tools CLI not found. Install with 'cargo install wasm-tools'." >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
# The source is already a full component in WAT; assemble it to a binary
# component with `parse` (not `component new`, which wraps a *core* module).
wasm-tools parse "$WAT" -o "$OUT_FILE"
wasm-tools validate "$OUT_FILE"
echo "wrote $OUT_FILE"
