#!/bin/bash
# Refresh the vendored WIT dependency copies from their canonical sources.
#
# pkcs11-wit and keys-wit are the single source of truth. The wasmtime host
# vendors copies under hosts/wasmtime/wit/keystore/deps so its bindgen
# builds standalone. Run this after changing the canonical WIT to avoid
# drift.
#
# Sources default to sibling checkouts; override with env vars:
#   PKCS11_WIT=/path/to/pkcs11-wit  KEYS_WIT=/path/to/keys-wit  ./scripts/sync-wit.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKCS11_WIT="${PKCS11_WIT:-$ROOT/../pkcs11-wit}"
KEYS_WIT="${KEYS_WIT:-$ROOT/../keys-wit}"

[ -d "$PKCS11_WIT" ] || { echo "pkcs11-wit not found at $PKCS11_WIT (set PKCS11_WIT)"; exit 1; }
[ -d "$KEYS_WIT" ]   || { echo "keys-wit not found at $KEYS_WIT (set KEYS_WIT)"; exit 1; }

refresh_deps() {
  local deps="$1"
  [ -d "$deps" ] || return 0
  for dir in "$deps"/*/; do
    local name; name="$(basename "$dir")"
    if [ -d "$PKCS11_WIT/$name" ]; then
      rm -rf "$dir"; cp -R "$PKCS11_WIT/$name" "$dir"
      echo "  $deps/$name <- pkcs11-wit"
    elif [ "$name" = "keys-keystore" ]; then
      rm -rf "$dir"; cp -R "$KEYS_WIT/keys-keystore" "$dir"
      echo "  $deps/$name <- keys-wit"
    fi
  done
}

echo "Syncing vendored WIT from:"
echo "  pkcs11-wit = $PKCS11_WIT"
echo "  keys-wit   = $KEYS_WIT"
refresh_deps "$ROOT/hosts/wasmtime/wit/keystore/deps"
echo "Done."
