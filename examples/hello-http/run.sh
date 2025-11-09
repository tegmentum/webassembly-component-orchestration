#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Build if not already built
WASM_FILE="$PROJECT_ROOT/target/wasm32-wasip2/release/hello_http.wasm"
if [ ! -f "$WASM_FILE" ]; then
    echo "Component not built. Building..."
    cd "$SCRIPT_DIR"
    ./build.sh
fi

echo "Running hello-http example..."
echo ""

# Create a test plan
PLAN_JSON=$(cat <<PLAN
{
  "version": "1",
  "root": "hello-http",
  "components": [
    {
      "id": "hello-http",
      "digest": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
      "source": null
    }
  ],
  "bindings": [],
  "secrets": [],
  "policy": {
    "max_memory_bytes": 52428800,
    "max_execution_time_ms": 30000,
    "allowed_imports": ["wasi:http", "wasi:io", "wasi:clocks"],
    "allowed_exports": ["wasi:http/incoming-handler"],
    "network_access": "listen",
    "filesystem_access": "none"
  }
}
PLAN
)

echo "=== Starting HTTP Server ==="
echo "The server will handle HTTP requests and demonstrate:"
echo "  - HTTP request routing"
echo "  - JSON and text responses"
echo "  - Logging patterns (INFO, DEBUG, METRIC)"
echo "  - Health check endpoint"
echo ""

# Check if wasmtime with HTTP support is available
if command -v wasmtime &> /dev/null; then
    WASMTIME_VERSION=$(wasmtime --version)
    echo "Using: $WASMTIME_VERSION"
    echo ""

    # Check if wasmtime serve is available
    if wasmtime serve --help &> /dev/null 2>&1; then
        echo "Starting HTTP server on http://localhost:8080"
        echo "Press Ctrl+C to stop"
        echo ""
        echo "Available endpoints:"
        echo "  GET  /        - Hello message"
        echo "  GET  /hello   - JSON response"
        echo "  GET  /health  - Health check"
        echo ""
        echo "Try: curl http://localhost:8080/hello"
        echo ""
        wasmtime serve "$WASM_FILE" --addr 127.0.0.1:8080
    else
        echo "Note: This wasmtime version doesn't support 'serve' command."
        echo "The hello-http component is built and ready at:"
        echo "  $WASM_FILE"
        echo ""
        echo "To run an HTTP server, you need:"
        echo "  - Wasmtime with HTTP support, or"
        echo "  - A compositor host with HTTP capabilities"
        echo ""
        echo "Example invocation (when available):"
        echo "  wasmtime serve $WASM_FILE --addr 127.0.0.1:8080"
    fi
else
    echo "Error: wasmtime not found. Install from: https://wasmtime.dev/"
    echo ""
    echo "The component is built at: $WASM_FILE"
    exit 1
fi

echo ""
echo "=== Example Complete ==="
