#!/usr/bin/env bash
# Package WIT bundles for OCI distribution
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
WIT_DIR="$PROJECT_ROOT/wit"
DIST_DIR="$PROJECT_ROOT/target/dist"

# Color output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log() {
    echo -e "${GREEN}[package-wit]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[package-wit]${NC} $1"
}

# Parse arguments
VERSION="${1:-v1.0.0}"
OUTPUT_DIR="${2:-$DIST_DIR}"

log "Packaging WIT bundles for version: $VERSION"
log "Output directory: $OUTPUT_DIR"

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Get git commit hash (or use timestamp if not in git repo)
if git rev-parse --git-dir > /dev/null 2>&1; then
    if git rev-parse HEAD > /dev/null 2>&1; then
        GIT_COMMIT=$(git rev-parse HEAD)
    else
        GIT_COMMIT="unknown"
    fi
else
    GIT_COMMIT="unknown"
fi

# Function to package a WIT directory
package_wit_bundle() {
    local wit_package=$1
    local package_dir="$WIT_DIR/$wit_package"
    local output_tar="$OUTPUT_DIR/${wit_package}-${VERSION}.tar"

    if [[ ! -d "$package_dir" ]]; then
        warn "Skipping $wit_package (directory not found)"
        return
    fi

    log "Packaging $wit_package..."

    # Create tar bundle with normalized timestamps for reproducibility
    # BSD tar (macOS) vs GNU tar compatibility
    if tar --version 2>&1 | grep -q "GNU tar"; then
        # GNU tar (Linux)
        tar -C "$WIT_DIR" -cf "$output_tar" \
            --sort=name \
            --mtime="2024-01-01 00:00:00" \
            --owner=0 --group=0 --numeric-owner \
            --pax-option=exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime \
            "$wit_package"
    else
        # BSD tar (macOS) - use find for sorted list
        (cd "$WIT_DIR" && find "$wit_package" -type f | sort | \
            tar -cf "$output_tar" --no-recursion -T -)
    fi

    # Compute digest (compatible with both sha256sum and shasum)
    if command -v sha256sum &> /dev/null; then
        local digest=$(sha256sum "$output_tar" | awk '{print $1}')
    else
        local digest=$(shasum -a 256 "$output_tar" | awk '{print $1}')
    fi

    log "  Created: $(basename "$output_tar")"
    log "  Digest: $digest"
    log "  Size: $(du -h "$output_tar" | cut -f1)"

    # Create metadata file
    cat > "$output_tar.metadata.json" <<EOF
{
  "package": "$wit_package",
  "version": "$VERSION",
  "digest": "sha256:$digest",
  "git_commit": "$GIT_COMMIT",
  "created_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "content_type": "application/vnd.wit.bundle.v1+tar"
}
EOF
}

# Package each WIT directory
log "Discovering WIT packages..."
for wit_pkg in "$WIT_DIR"/*; do
    if [[ -d "$wit_pkg" && ! "$(basename "$wit_pkg")" =~ ^(canon-|README) ]]; then
        package_wit_bundle "$(basename "$wit_pkg")"
    fi
done

# Create manifest of all packages
log "Creating bundle manifest..."
cat > "$OUTPUT_DIR/manifest.json" <<EOF
{
  "version": "$VERSION",
  "git_commit": "$GIT_COMMIT",
  "created_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "bundles": [
EOF

# Add bundle entries
first=true
for tar_file in "$OUTPUT_DIR"/*.tar; do
    if [[ -f "$tar_file.metadata.json" ]]; then
        if [[ "$first" == "false" ]]; then
            echo "," >> "$OUTPUT_DIR/manifest.json"
        fi
        first=false
        cat "$tar_file.metadata.json" >> "$OUTPUT_DIR/manifest.json"
    fi
done

cat >> "$OUTPUT_DIR/manifest.json" <<EOF

  ]
}
EOF

log "Bundle manifest created: $OUTPUT_DIR/manifest.json"
log ""
log "Summary:"
log "  Version: $VERSION"
log "  Git commit: $GIT_COMMIT"
log "  Total bundles: $(ls -1 "$OUTPUT_DIR"/*.tar 2>/dev/null | wc -l)"
log ""
log "Next steps:"
log "  1. Review bundles in: $OUTPUT_DIR"
log "  2. Generate provenance: ./scripts/release/generate-provenance.sh $VERSION"
log "  3. Sign and push to OCI registry"
