#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Building hello-http example..."

# Check if wit-deps is installed
if ! command -v wit-deps &> /dev/null; then
    echo "Installing wit-deps..."
    cargo install wit-deps-cli
fi

# Fetch WASI HTTP dependencies
cd "$SCRIPT_DIR"
if [ ! -d "wit/deps" ]; then
    echo "Fetching WASI dependencies..."
    cat > wit/deps.toml <<EOF
http = { url = "https://github.com/WebAssembly/wasi-http/archive/v0.2.1.tar.gz" }
io = { url = "https://github.com/WebAssembly/wasi-io/archive/v0.2.1.tar.gz" }
clocks = { url = "https://github.com/WebAssembly/wasi-clocks/archive/v0.2.1.tar.gz" }
random = { url = "https://github.com/WebAssembly/wasi-random/archive/v0.2.1.tar.gz" }
cli = { url = "https://github.com/WebAssembly/wasi-cli/archive/v0.2.1.tar.gz" }
EOF
    wit-deps
fi

# Build the component
echo "Building WebAssembly component..."
cargo build --release --target wasm32-wasip2

echo "Build complete!"
echo "Component: target/wasm32-wasip2/release/hello_http.wasm"
