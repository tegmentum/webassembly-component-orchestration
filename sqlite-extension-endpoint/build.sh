#!/usr/bin/env bash
# Build the sqlite-extension-endpoint artifacts and compose one provider
# per declarative tier with a real sqlink catalog extension.
#
#   1. build each provider SHAPE (one Cargo feature each, same source),
#   2. (re)build the test extensions at sqlite:extension@1.0.0 and
#      componentize them with the wasi reactor adapter,
#   3. `wac plug` each shape with its extension -> <ext>-provider.wasm
#      (a valid compose:dynlink resident provider),
#   4. build the generic dlopen harness guest.
#
# Outputs land in $OUT (default ./dist). Override extension sources with
# SQLINK_ROOT. Idempotent; safe to re-run.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE"

SQLINK_ROOT="${SQLINK_ROOT:-$HOME/git/sqlink}"
OUT="${OUT:-$HERE/dist}"
ADAPTER="${WASI_ADAPTER:-$HOME/.cache/xtran/wasi_snapshot_preview1.reactor.wasm}"
PROV_WASM="provider/target/wasm32-wasip2/release/sqlite_extension_endpoint.wasm"

mkdir -p "$OUT/components" "$OUT/providers"

# shape -> (extension crate, built-module-name, component-output-name)
# Every shape compiles the SAME provider source; only the world differs.
declare -a TIERS=(
  "scalar:aba:aba_extension"
  "aggregate:count_min:count_min_extension"
  "collation:uint:uint_extension"
  "vtab:series:series_extension"
  "hooks:hookprobe:hookprobe_extension"
  "dotcmd:greet:greet_extension"
)

componentize() {
  # $1 = built wasm (module or component), $2 = output component path
  local src="$1" dst="$2"
  if wasm-tools component wit "$src" 2>/dev/null | grep -q 'root:root'; then
    wasm-tools component new "$src" --adapt "wasi_snapshot_preview1=$ADAPTER" -o "$dst"
  else
    cp "$src" "$dst"
  fi
}

build_extension() {
  # $1 = crate dir name, $2 = built module basename. Build at @1.0.0 into
  # an isolated target so we never disturb the sqlink working tree.
  local crate="$1" base="$2"
  local extdir="$SQLINK_ROOT/extensions/$crate"
  local shared="$SQLINK_ROOT/extensions/_shared-target"
  # Prefer an already-built @1.0.0 component in _shared-target.
  local pre="$shared/wasm32-wasip2/release/${base}.component.wasm"
  if [[ -f "$pre" ]]; then
    local ver
    ver=$(wasm-tools component wit "$pre" 2>/dev/null | grep -oE 'export sqlite:extension/metadata@[0-9.]+' | head -1)
    if [[ "$ver" == *"@1.0.0" ]]; then
      echo "$pre"
      return
    fi
  fi
  # Otherwise build into an isolated target dir.
  local td="$OUT/extbuild/$crate"
  mkdir -p "$td"
  ( cd "$extdir" && CARGO_TARGET_DIR="$td" cargo build --release --target wasm32-wasip2 >/dev/null 2>&1 )
  echo "$td/wasm32-wasip2/release/${base}.wasm"
}

echo "==> Building generic dlopen harness"
( cd harness && cargo build --release --target wasm32-wasip2 )

for entry in "${TIERS[@]}"; do
  IFS=: read -r shape crate base <<< "$entry"
  echo "==> [$shape] provider + $crate"
  ( cd provider && cargo build --release --target wasm32-wasip2 \
      --no-default-features --features "$shape" >/dev/null )

  built="$(build_extension "$crate" "$base")"
  comp="$OUT/components/${crate}.wasm"
  componentize "$built" "$comp"

  out="$OUT/providers/${crate}-provider.wasm"
  if [[ "$shape" == "dotcmd" ]]; then
    # The dot-command cli-stdout dependency is cyclic (provider provides
    # cli-stdout, consumes dot-command) and wac cannot express the cycle.
    # Left for host-mediated wiring / the reentrant tier (#220).
    echo "    (dotcmd: provider built; wac-cyclic compose deferred — see REPORT)"
    cp "$comp" "$OUT/components/greet.wasm"
    continue
  fi
  wac plug --plug "$comp" "$PROV_WASM" -o "$out"
  left=$(wasm-tools component wit "$out" 2>/dev/null \
    | grep -E '^  import sqlite:extension/[a-z-]+@' | grep -vE 'types|policy' || true)
  echo "    -> $(basename "$out") (leftover host imports: ${left:-none})"
done

echo "==> Done. providers in $OUT/providers, harness:"
echo "    harness/target/wasm32-wasip2/release/sqlite-ext-endpoint-harness.wasm"
