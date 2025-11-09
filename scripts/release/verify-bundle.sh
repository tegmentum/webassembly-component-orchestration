#!/usr/bin/env bash
# Verify WIT bundle integrity and provenance
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DIST_DIR="$PROJECT_ROOT/target/dist"

# Color output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

log() {
    echo -e "${GREEN}[verify]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[verify]${NC} $1"
}

error() {
    echo -e "${RED}[verify]${NC} $1"
}

# Parse arguments
BUNDLE_PATH="${1:-}"
OUTPUT_DIR="${2:-$DIST_DIR}"

if [[ -z "$BUNDLE_PATH" ]]; then
    echo "Usage: $0 <bundle.tar> [dist-dir]"
    echo ""
    echo "Verifies:"
    echo "  - Bundle integrity (digest matches metadata)"
    echo "  - Contents are valid WIT files"
    echo "  - Provenance includes bundle"
    exit 1
fi

if [[ ! -f "$BUNDLE_PATH" ]]; then
    error "Bundle not found: $BUNDLE_PATH"
    exit 1
fi

BUNDLE_NAME=$(basename "$BUNDLE_PATH")
log "Verifying bundle: $BUNDLE_NAME"

# Step 1: Verify digest matches metadata
log "Checking digest..."
if [[ -f "$BUNDLE_PATH.metadata.json" ]]; then
    EXPECTED_DIGEST=$(jq -r '.digest' "$BUNDLE_PATH.metadata.json" | cut -d: -f2)

    if command -v sha256sum &> /dev/null; then
        ACTUAL_DIGEST=$(sha256sum "$BUNDLE_PATH" | awk '{print $1}')
    else
        ACTUAL_DIGEST=$(shasum -a 256 "$BUNDLE_PATH" | awk '{print $1}')
    fi

    if [[ "$EXPECTED_DIGEST" == "$ACTUAL_DIGEST" ]]; then
        log "  ✓ Digest matches: $ACTUAL_DIGEST"
    else
        error "  ✗ Digest mismatch!"
        error "    Expected: $EXPECTED_DIGEST"
        error "    Actual:   $ACTUAL_DIGEST"
        exit 1
    fi
else
    warn "  No metadata file found, skipping digest check"
fi

# Step 2: Check contents
log "Checking bundle contents..."
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

tar -xf "$BUNDLE_PATH" -C "$TEMP_DIR"

# Count WIT files
WIT_COUNT=$(find "$TEMP_DIR" -name "*.wit" | wc -l | tr -d ' ')
if [[ "$WIT_COUNT" -gt 0 ]]; then
    log "  ✓ Found $WIT_COUNT WIT file(s)"

    # List WIT files
    find "$TEMP_DIR" -name "*.wit" | while read -r wit_file; do
        REL_PATH=$(echo "$wit_file" | sed "s|$TEMP_DIR/||")
        SIZE=$(du -h "$wit_file" | cut -f1)
        log "    - $REL_PATH ($SIZE)"
    done
else
    error "  ✗ No WIT files found in bundle"
    exit 1
fi

# Step 3: Check if bundle is in provenance (if available)
if [[ -f "$OUTPUT_DIR/provenance.json" ]]; then
    log "Checking provenance..."

    if jq -e ".subject[] | select(.name == \"$BUNDLE_NAME\")" "$OUTPUT_DIR/provenance.json" > /dev/null; then
        PROV_DIGEST=$(jq -r ".subject[] | select(.name == \"$BUNDLE_NAME\") | .digest.sha256" "$OUTPUT_DIR/provenance.json")
        log "  ✓ Bundle found in provenance"
        log "    Provenance digest: $PROV_DIGEST"

        if [[ "$PROV_DIGEST" == "$ACTUAL_DIGEST" ]]; then
            log "  ✓ Provenance digest matches bundle"
        else
            warn "  Provenance digest doesn't match (bundle may have been regenerated)"
        fi
    else
        warn "  Bundle not found in provenance"
    fi
else
    warn "No provenance file found at $OUTPUT_DIR/provenance.json"
fi

# Step 4: Summary
log ""
log "Verification complete!"
log "  Bundle: $BUNDLE_NAME"
log "  Digest: $ACTUAL_DIGEST"
log "  WIT files: $WIT_COUNT"

# Show metadata if available
if [[ -f "$BUNDLE_PATH.metadata.json" ]]; then
    log ""
    log "Metadata:"
    jq '.' "$BUNDLE_PATH.metadata.json" | sed 's/^/  /'
fi
