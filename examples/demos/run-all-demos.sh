#!/usr/bin/env bash
# Run all demonstrations
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Running all demonstrations..."
echo ""

demos=(
    "secrets-demo.sh"
    "trust-demo.sh"
    "determinism-demo.sh"
)

for demo in "${demos[@]}"; do
    echo ""
    echo "========================================"
    echo "Running: $demo"
    echo "========================================"
    echo ""
    "$SCRIPT_DIR/$demo"
    echo ""
    read -p "Press Enter to continue to next demo..."
done

echo ""
echo "All demonstrations complete!"
