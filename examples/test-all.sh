#!/usr/bin/env bash
# Test all examples and demos
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Testing all examples and demos..."
echo ""

# Test hello-cli
echo "Testing hello-cli..."
cd "$SCRIPT_DIR/hello-cli"
OUTPUT=$(./run.sh "TestUser" 2>&1)
if echo "$OUTPUT" | grep -q "Hello"; then
    echo "  ✓ hello-cli works"
else
    echo "  ✗ hello-cli failed"
    echo "Output was:"
    echo "$OUTPUT"
    exit 1
fi

# Test demos
echo ""
echo "Testing demos..."
cd "$SCRIPT_DIR/demos"

for demo in secrets-demo.sh trust-demo.sh determinism-demo.sh; do
    if ./"$demo" > /dev/null 2>&1; then
        echo "  ✓ $demo works"
    else
        echo "  ✗ $demo failed"
        exit 1
    fi
done

echo ""
echo "All tests passed!"
