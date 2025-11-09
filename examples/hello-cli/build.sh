#!/usr/bin/env bash
set -euo pipefail

echo "Building hello-cli example..."
cargo build --release --target wasm32-wasip1
echo "Build complete!"
