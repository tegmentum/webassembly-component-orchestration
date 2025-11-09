#!/usr/bin/env bash
# Push WIT bundles to OCI registry
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DIST_DIR="$PROJECT_ROOT/target/dist"

# Color output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log() {
    echo -e "${GREEN}[oci-push]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[oci-push]${NC} $1"
}

info() {
    echo -e "${BLUE}[oci-push]${NC} $1"
}

# Parse arguments
VERSION="${1:-}"
REGISTRY="${2:-}"

if [[ -z "$VERSION" || -z "$REGISTRY" ]]; then
    echo "Usage: $0 <version> <registry>"
    echo ""
    echo "Example:"
    echo "  $0 v1.0.0 registry.example.com/wit"
    echo ""
    echo "Environment variables:"
    echo "  OCI_USERNAME - Registry username"
    echo "  OCI_PASSWORD - Registry password"
    echo "  DRY_RUN=1    - Don't actually push, just show what would be done"
    exit 1
fi

OUTPUT_DIR="${3:-$DIST_DIR}"
DRY_RUN="${DRY_RUN:-0}"

log "Pushing WIT bundles to OCI registry"
log "  Version: $VERSION"
log "  Registry: $REGISTRY"
log "  Source: $OUTPUT_DIR"

if [[ "$DRY_RUN" == "1" ]]; then
    warn "DRY RUN MODE - No actual pushes will be performed"
fi

# Check if manifest exists
if [[ ! -f "$OUTPUT_DIR/manifest.json" ]]; then
    echo "Error: No manifest found at $OUTPUT_DIR/manifest.json"
    echo "Run package-wit.sh first."
    exit 1
fi

# Check for required tools
if ! command -v oras &> /dev/null; then
    warn "oras CLI not found. Install from: https://oras.land/"
    info "Alternative: Using docker/podman for OCI push"
    USE_DOCKER=1
else
    USE_DOCKER=0
fi

# Login to registry if credentials provided
if [[ -n "${OCI_USERNAME:-}" && -n "${OCI_PASSWORD:-}" ]]; then
    log "Logging in to registry..."
    if [[ "$DRY_RUN" != "1" ]]; then
        if [[ "$USE_DOCKER" == "1" ]]; then
            echo "$OCI_PASSWORD" | docker login "$REGISTRY" -u "$OCI_USERNAME" --password-stdin
        else
            echo "$OCI_PASSWORD" | oras login "$REGISTRY" -u "$OCI_USERNAME" --password-stdin
        fi
    fi
else
    info "No registry credentials provided (OCI_USERNAME/OCI_PASSWORD)"
    info "Assuming registry allows anonymous push or already logged in"
fi

# Function to push a bundle using oras
push_bundle_oras() {
    local tar_file=$1
    local package_name=$(basename "$tar_file" | sed "s/-${VERSION}.tar$//")
    local oci_ref="${REGISTRY}/${package_name}:${VERSION}"

    log "Pushing $package_name to $oci_ref"

    if [[ "$DRY_RUN" == "1" ]]; then
        info "  [DRY RUN] Would push: $tar_file"
        info "  [DRY RUN] To: $oci_ref"
        return
    fi

    # Push using oras with WIT bundle media type
    oras push "$oci_ref" \
        "$tar_file:application/vnd.wit.bundle.v1+tar" \
        "$tar_file.metadata.json:application/vnd.oci.image.manifest.v1+json"

    local digest=$(oras manifest fetch "$oci_ref" --descriptor | jq -r '.digest')
    log "  Pushed successfully"
    log "  Digest: $digest"
    log "  Pull: oras pull $oci_ref"
}

# Function to push manifest and provenance
push_metadata() {
    local manifest_ref="${REGISTRY}/manifest:${VERSION}"
    local provenance_ref="${REGISTRY}/provenance:${VERSION}"

    log "Pushing manifest to $manifest_ref"
    if [[ "$DRY_RUN" != "1" ]]; then
        oras push "$manifest_ref" \
            "$OUTPUT_DIR/manifest.json:application/json"
    else
        info "  [DRY RUN] Would push manifest"
    fi

    if [[ -f "$OUTPUT_DIR/provenance.json" ]]; then
        log "Pushing provenance to $provenance_ref"
        if [[ "$DRY_RUN" != "1" ]]; then
            oras push "$provenance_ref" \
                "$OUTPUT_DIR/provenance.json:application/vnd.in-toto+json"
        else
            info "  [DRY RUN] Would push provenance"
        fi
    fi
}

# Push each bundle
log "Pushing WIT bundles..."
for tar_file in "$OUTPUT_DIR"/*.tar; do
    if [[ -f "$tar_file" ]]; then
        if [[ "$USE_DOCKER" == "1" ]]; then
            warn "Docker-based push not yet implemented. Please install oras."
            warn "Download from: https://github.com/oras-project/oras/releases"
            exit 1
        else
            push_bundle_oras "$tar_file"
        fi
    fi
done

# Push metadata
if [[ "$USE_DOCKER" != "1" ]]; then
    push_metadata
fi

log ""
log "OCI push complete!"
log ""
info "Verify with:"
info "  oras discover ${REGISTRY}/std-secrets:${VERSION}"
info "  oras pull ${REGISTRY}/std-secrets:${VERSION}"
