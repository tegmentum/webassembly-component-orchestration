#!/usr/bin/env bash
# Secrets Management Demo
set -euo pipefail

echo "================================================"
echo "Secrets Management Demo"
echo "================================================"
echo ""

echo "This demo shows how secrets are injected into components:"
echo ""

echo "1. Environment Variable Secrets"
echo "   - Secrets passed as environment variables"
echo "   - Component reads from process environment"
echo ""

cat <<PLAN
Example Plan:
{
  "secrets": [
    {
      "id": "api-key",
      "type": "env",
      "target_component": "backend",
      "target_name": "API_KEY"
    }
  ]
}
PLAN

echo ""
echo "2. Vault Integration (Simulated)"
echo "   - Secrets fetched from HashiCorp Vault"
echo "   - Retrieved at composition time"
echo ""

cat <<VAULT
vault kv get -field=value secret/myapp/api-key
Injected as: API_KEY=sk_...
VAULT

echo ""
echo "3. PKCS#11 Key Access (Simulated)"
echo "   - Cryptographic keys from HSM/TPM"
echo "   - No key material in component"
echo ""

cat <<PKCS
Component calls: sign(data)
Host uses: PKCS#11 token for signing
Returns: signature
PKCS

echo ""
echo "================================================"
echo "Demo Complete"
echo "================================================"
echo ""
echo "Key Features:"
echo "  ✓ Multiple secret backends (env, vault, pkcs11)"
echo "  ✓ Secrets never in plan file"
echo "  ✓ Least-privilege access"
echo "  ✓ Audit trail of secret access"
echo ""
echo "See: hosts/wasmtime/src/secrets/ for implementation"
