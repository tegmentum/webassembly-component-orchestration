#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# This crate is excluded from the workspace (it targets wasm32-wasip2),
# so it builds to its own crate-local target/ directory.
TARGET="$SCRIPT_DIR/target/wasm32-wasip2/release"
RAW="$TARGET/compose_orchestrator_wasm.wasm"
COMPOSED="$TARGET/compose_orchestrator_composed.wasm"

# The composed secure-log component that satisfies the orchestrator's
# secure-log:log/log import. Build it with secure-log's
# scripts/build-components.sh; override the location with SECURE_LOG_WASM.
SECURE_LOG_WASM="${SECURE_LOG_WASM:-$SCRIPT_DIR/../../../secure-log/dist/secure-log-sqlite.wasm}"

echo "==> Building compose-orchestrator-wasm (wasm32-wasip2, release)"
cargo build --release --target wasm32-wasip2
echo "    raw artifact: $RAW"

echo "==> Composing with secure-log (wac plug)"
if [[ -f "$SECURE_LOG_WASM" ]]; then
    wac plug --plug "$SECURE_LOG_WASM" "$RAW" -o "$COMPOSED"
    echo "    composed artifact: $COMPOSED"
else
    echo "    !! secure-log component not found at $SECURE_LOG_WASM"
    echo "       Build it: (cd ../../../secure-log && ./scripts/build-components.sh)"
    echo "       or set SECURE_LOG_WASM=<path>. Skipping composition."
fi

if command -v wasm-tools &> /dev/null && [[ -f "$COMPOSED" ]]; then
    echo
    echo "World declared by the composed component:"
    wasm-tools component wit "$COMPOSED" | grep -E '^world|import |export ' | sed 's/^/    /'
fi
