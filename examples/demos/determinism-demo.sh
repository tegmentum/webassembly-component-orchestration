#!/usr/bin/env bash
# Determinism Mode Demo
set -euo pipefail

echo "================================================"
echo "Determinism Mode Demo"
echo "================================================"
echo ""

echo "This demo shows deterministic vs non-deterministic execution:"
echo ""

echo "Mode 1: Strict Determinism"
echo "-------------------------------------------"
cat <<STRICT
Policy:
  determinism_mode: "strict"
  allowed_syscalls: []
  
Component Behavior:
  - time() → ERROR: syscall not allowed
  - random() → ERROR: syscall not allowed
  - filesystem → ERROR: I/O not allowed
  
Result: Fully reproducible execution
Hash: Same input → Same output → Same execution trace
STRICT

echo ""
echo "Mode 2: Relaxed Determinism"
echo "-------------------------------------------"
cat <<RELAXED
Policy:
  determinism_mode: "relaxed"
  allowed_syscalls: ["time", "random"]
  
Component Behavior:
  - time() → OK: returns current time
  - random() → OK: returns random bytes
  - filesystem → ALLOWED: reads permitted
  
Result: Non-deterministic but auditable
Hash: Same input → Different output (timestamps/random)
RELAXED

echo ""
echo "Use Cases:"
echo "-------------------------------------------"
echo "Strict Mode:"
echo "  - Reproducible builds"
echo "  - Formal verification"
echo "  - Distributed consensus"
echo ""
echo "Relaxed Mode:"
echo "  - Time-based operations"
echo "  - Cryptographic key generation"
echo "  - Real-world applications"
echo ""

echo "================================================"
echo "Demo Complete"
echo "================================================"
echo ""
echo "Key Features:"
echo "  ✓ Configurable determinism levels"
echo "  ✓ Syscall filtering"
echo "  ✓ Reproducible execution for strict mode"
echo "  ✓ Audit trail in both modes"
echo ""
echo "See: hosts/wasmtime/src/exec.rs for policy enforcement"
