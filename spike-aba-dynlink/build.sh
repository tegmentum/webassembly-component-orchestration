#!/usr/bin/env bash
# Build the aba-dynlink spike artifacts:
#   1. the aba-endpoint adapter (exports compose:dynlink/endpoint, imports
#      sqlite:extension SPI),
#   2. compose it with the real aba sqlite:extension component (wac plug)
#      -> aba-provider.wasm  (a valid compose:dynlink resident provider),
#   3. the flavor-B dlopen harness guest.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

ABA="${ABA_WASM:-$HOME/git/sqlink/extensions/aba/target/wasm32-wasip2/release/aba_extension.component.wasm}"
if [[ ! -f "$ABA" ]]; then
  echo "aba component not found at $ABA" >&2
  echo "build it: (cd ~/git/sqlink/extensions/aba && cargo build --release --target wasm32-wasip2)" >&2
  exit 1
fi

echo "==> Building aba-endpoint adapter"
(cd aba-endpoint && cargo build --release --target wasm32-wasip2)
ADAPTER="aba-endpoint/target/wasm32-wasip2/release/aba_endpoint.wasm"

echo "==> Composing adapter + aba -> aba-provider.wasm (wac plug)"
wac plug --plug "$ABA" "$ADAPTER" -o aba-provider.wasm

echo "==> Building dlopen harness guest"
(cd harness && cargo build --release --target wasm32-wasip2)

echo "==> Done:"
echo "    provider: $SCRIPT_DIR/aba-provider.wasm"
echo "    harness:  $SCRIPT_DIR/harness/target/wasm32-wasip2/release/aba-dlopen-harness.wasm"
