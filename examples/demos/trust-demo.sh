#!/usr/bin/env bash
# Trust & Signature Verification Demo  
set -euo pipefail

echo "================================================"
echo "Trust & Signature Verification Demo"
echo "================================================"
echo ""

echo "This demo shows component trust verification:"
echo ""

echo "Scenario 1: Unsigned Component (Rejected)"
echo "-------------------------------------------"
cat <<UNSIGNED
Component: unsigned-app.wasm
Digest: sha256:abc123...
Signature: NONE

Policy Check: FAILED
Error: Component not signed, policy requires signature
UNSIGNED

echo ""
echo "Scenario 2: Signed Component (Accepted)"
echo "-------------------------------------------"
cat <<SIGNED
Component: signed-app.wasm
Digest: sha256:def456...
Signature: RSA-PSS (valid)
Signer: CN=TrustedDev, O=Company

Trust Check: PASSED
- Signature valid ✓
- Certificate chains to trusted root ✓
- Not revoked ✓
SIGNED

echo ""
echo "Trust Backends:"
echo "-------------------------------------------"
echo "1. SigStore (Keyless signing)"
echo "   - OIDC identity verification"
echo "   - Transparency log (Rekor)"
echo ""
echo "2. PGP/GPG"
echo "   - Traditional PGP signatures"
echo "   - Web of trust model"
echo ""
echo "3. X.509 PKI"
echo "   - Certificate-based signing"
echo "   - Enterprise CA integration"
echo ""

echo "================================================"
echo "Demo Complete"
echo "================================================"
echo ""
echo "Key Features:"
echo "  ✓ Multiple trust backends"
echo "  ✓ Signature verification before execution"
echo "  ✓ Policy-based trust decisions"
echo "  ✓ Revocation checking"
echo ""
echo "See: hosts/wasmtime/src/trust/ for implementation"
