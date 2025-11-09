#!/usr/bin/env bash
# Complete release automation script
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Color output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

log() {
    echo -e "${GREEN}[release]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[release]${NC} $1"
}

info() {
    echo -e "${BLUE}[release]${NC} $1"
}

error() {
    echo -e "${RED}[release]${NC} $1"
}

# Parse arguments
VERSION="${1:-}"
REGISTRY="${2:-}"

if [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <version> [registry]"
    echo ""
    echo "Example:"
    echo "  $0 v1.0.0 registry.example.com/wit"
    echo ""
    echo "Steps:"
    echo "  1. Package WIT bundles"
    echo "  2. Generate provenance"
    echo "  3. Create git tag (if in git repo)"
    echo "  4. Push to OCI registry (if registry provided)"
    exit 1
fi

log "Starting release process for $VERSION"
log "Project: $(basename "$PROJECT_ROOT")"

# Step 1: Package WIT bundles
log ""
log "Step 1: Packaging WIT bundles..."
"$SCRIPT_DIR/package-wit.sh" "$VERSION"

if [[ $? -ne 0 ]]; then
    error "Failed to package WIT bundles"
    exit 1
fi

# Step 2: Generate provenance
log ""
log "Step 2: Generating provenance..."
"$SCRIPT_DIR/generate-provenance.sh" "$VERSION"

if [[ $? -ne 0 ]]; then
    error "Failed to generate provenance"
    exit 1
fi

# Step 3: Create git tag (if in git repo)
log ""
log "Step 3: Git tagging..."
if git rev-parse --git-dir > /dev/null 2>&1; then
    if ! git rev-parse HEAD > /dev/null 2>&1; then
        warn "No commits yet, skipping tag creation"
        info "Initialize repository with:"
        info "  git add ."
        info "  git commit -m 'Initial commit'"
        info "  git tag -a $VERSION -m 'Release $VERSION'"
    elif git tag -l "$VERSION" | grep -q "$VERSION"; then
        warn "Tag $VERSION already exists"
    else
        log "Creating git tag: $VERSION"
        git tag -a "$VERSION" -m "Release $VERSION"
        log "Tag created successfully"
        info "Push tag with: git push origin $VERSION"
    fi
else
    warn "Not in a git repository, skipping tag creation"
fi

# Step 4: Push to OCI registry (if provided)
if [[ -n "$REGISTRY" ]]; then
    log ""
    log "Step 4: Pushing to OCI registry..."

    # Check for oras
    if ! command -v oras &> /dev/null; then
        warn "oras CLI not found, skipping OCI push"
        warn "Install from: https://oras.land/"
        info "You can manually push later with:"
        info "  $SCRIPT_DIR/push-oci.sh $VERSION $REGISTRY"
    else
        "$SCRIPT_DIR/push-oci.sh" "$VERSION" "$REGISTRY"

        if [[ $? -ne 0 ]]; then
            error "Failed to push to OCI registry"
            exit 1
        fi
    fi
else
    warn "No registry provided, skipping OCI push"
    info "To push to OCI registry later:"
    info "  $SCRIPT_DIR/push-oci.sh $VERSION <registry>"
fi

# Summary
log ""
log "========================================="
log "Release $VERSION complete!"
log "========================================="
log ""
log "Artifacts location: $PROJECT_ROOT/target/dist"
log ""

if [[ -n "$REGISTRY" ]] && command -v oras &> /dev/null; then
    log "Pull bundles with:"
    log "  oras pull ${REGISTRY}/std-secrets:${VERSION}"
    log "  oras pull ${REGISTRY}/std-attest:${VERSION}"
    log "  oras pull ${REGISTRY}/sys-compose:${VERSION}"
    log ""
fi

log "Verify provenance with:"
log "  cat target/dist/provenance.json | jq ."
log ""

if git rev-parse --git-dir > /dev/null 2>&1; then
    log "Push git tag with:"
    log "  git push origin $VERSION"
    log ""
fi

log "Next steps:"
log "  1. Review artifacts in target/dist/"
log "  2. Sign attestation bundle (optional):"
log "     cosign sign-blob target/dist/attestation.json"
log "  3. Push git tag to remote"
log "  4. Create GitHub release"
