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

# Host stubs (see libs/host-stubs/README.md) satisfy the remaining
# non-WASI functional imports of the orchestrator:
#   compose:host/runtime-info@0.1.0    <- runtime-info-stub
#   host:bootstrap/bootstrap@0.1.0     <- bootstrap-stub
#   tegmentum:runtime/control@0.1.0    <- runtime-control-stub
# All three are built via cargo-component in the host-stubs workspace.
# Override individual paths with the corresponding env vars for
# consumers who want to substitute a real implementation.
HOST_STUBS_TARGET="$SCRIPT_DIR/../host-stubs/target/wasm32-wasip2/release"
RUNTIME_INFO_STUB_WASM="${RUNTIME_INFO_STUB_WASM:-$HOST_STUBS_TARGET/runtime_info_stub.wasm}"
BOOTSTRAP_STUB_WASM="${BOOTSTRAP_STUB_WASM:-$HOST_STUBS_TARGET/bootstrap_stub.wasm}"
RUNTIME_CONTROL_STUB_WASM="${RUNTIME_CONTROL_STUB_WASM:-$HOST_STUBS_TARGET/runtime_control_stub.wasm}"

echo "==> Building compose-orchestrator-wasm (wasm32-wasip2, release)"
cargo build --release --target wasm32-wasip2
echo "    raw artifact: $RAW"

echo "==> Building host stubs (wasm32-wasip2, release)"
if command -v cargo-component &> /dev/null; then
    ( cd "$SCRIPT_DIR/../host-stubs" && \
      cargo component build --release --target wasm32-wasip2 \
          -p runtime-info-stub \
          -p bootstrap-stub \
          -p runtime-control-stub )
    echo "    runtime-info-stub:    $RUNTIME_INFO_STUB_WASM"
    echo "    bootstrap-stub:       $BOOTSTRAP_STUB_WASM"
    echo "    runtime-control-stub: $RUNTIME_CONTROL_STUB_WASM"
else
    echo "    !! cargo-component not on PATH; skipping stub build."
    echo "       Install: cargo install cargo-component"
fi

# Assemble the wac plug chain. Each plug's exports satisfy imports on
# the base. Order does not matter for these stubs since none of them
# import each other, but we keep the secure-log-sqlite plug first for
# consistency with the plug chain that has been in this script the
# longest.
PLUGS=()
for wasm in "$SECURE_LOG_WASM" \
            "$RUNTIME_INFO_STUB_WASM" \
            "$BOOTSTRAP_STUB_WASM" \
            "$RUNTIME_CONTROL_STUB_WASM"; do
    if [[ -f "$wasm" ]]; then
        PLUGS+=(--plug "$wasm")
    else
        echo "    !! missing plug: $wasm"
    fi
done

echo "==> Composing orchestrator with plugs (wac plug)"
if [[ ${#PLUGS[@]} -gt 0 && -f "$RAW" ]]; then
    wac plug "${PLUGS[@]}" "$RAW" -o "$COMPOSED"
    echo "    composed artifact: $COMPOSED"
else
    echo "    !! No plugs found; skipping composition."
    echo "       Build secure-log: (cd ../../../secure-log && ./scripts/build-components.sh)"
    echo "       Build host stubs: (cd ../host-stubs && cargo component build --release --target wasm32-wasip2)"
fi

if command -v wasm-tools &> /dev/null && [[ -f "$COMPOSED" ]]; then
    echo
    echo "World declared by the composed component:"
    wasm-tools component wit "$COMPOSED" | grep -E '^world|import |export ' | sed 's/^/    /'
fi
