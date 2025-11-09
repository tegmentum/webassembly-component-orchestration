#!/usr/bin/env bash
# Build all examples
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Building all examples..."
echo ""

# Check for required tools
if ! command -v cargo &> /dev/null; then
    echo "Error: cargo not found. Install from: https://rustup.rs/"
    exit 1
fi

if ! rustc --print target-list | grep -q wasm32-wasip1; then
    echo "Installing wasm32-wasip1 target..."
    rustup target add wasm32-wasip1
fi

if ! rustc --print target-list | grep -q wasm32-wasip2; then
    echo "Installing wasm32-wasip2 target..."
    rustup target add wasm32-wasip2
fi

# Build hello-cli
echo "Building hello-cli..."
cd "$SCRIPT_DIR/hello-cli"
cargo build --release --target wasm32-wasip1
echo "  ✓ hello-cli built"

# Build hello-http
echo ""
echo "Building hello-http..."
cd "$SCRIPT_DIR/hello-http"
./build.sh
echo "  ✓ hello-http built"

echo ""
echo "All examples built successfully!"
echo ""
echo "Run examples:"
echo "  cd examples/hello-cli && ./run.sh"
echo "  cd examples/hello-http && ./run.sh"
echo ""
echo "Run demos:"
echo "  cd examples/demos && ./run-all-demos.sh"
