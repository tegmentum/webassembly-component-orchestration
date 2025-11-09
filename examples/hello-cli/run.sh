#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Build if not already built
WASM_FILE="$PROJECT_ROOT/target/wasm32-wasip1/release/hello-cli.wasm"
if [ ! -f "$WASM_FILE" ]; then
    echo "Component not built. Building..."
    "$SCRIPT_DIR/build.sh"
fi

echo "Running hello-cli example..."
echo ""

# Create a test plan
PLAN_JSON=$(cat <<PLAN
{
  "version": "1",
  "root": "hello-cli",
  "components": [
    {
      "id": "hello-cli",
      "digest": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
      "source": null
    }
  ],
  "bindings": [],
  "secrets": [],
  "policy": {
    "max_memory_bytes": 10485760,
    "max_execution_time_ms": 5000,
    "allowed_imports": [],
    "allowed_exports": [],
    "network_access": "none",
    "filesystem_access": "read-only"
  }
}
PLAN
)

# Run using wasmtime directly for demo purposes
# In production, would use the compositor host
echo "=== Output ==="
if command -v wasmtime &> /dev/null; then
    wasmtime run "$WASM_FILE" -- "$@"
else
    echo "Error: wasmtime not found. Install from: https://wasmtime.dev/"
    echo ""
    echo "Alternative: Use the compositor host:"
    echo "  cargo run --manifest-path $PROJECT_ROOT/hosts/wasmtime/Cargo.toml"
    exit 1
fi

echo ""
echo "=== Example Complete ==="
echo "This example demonstrates:"
echo "  - Basic CLI argument handling"
echo "  - Standard output and error streams"
echo "  - Exit codes"
echo "  - Logging patterns (INFO, DEBUG, METRIC)"
