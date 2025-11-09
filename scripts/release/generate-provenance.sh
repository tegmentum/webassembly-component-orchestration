#!/usr/bin/env bash
# Generate SLSA provenance for WIT bundles
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DIST_DIR="$PROJECT_ROOT/target/dist"

# Color output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

log() {
    echo -e "${GREEN}[provenance]${NC} $1"
}

info() {
    echo -e "${BLUE}[provenance]${NC} $1"
}

# Parse arguments
VERSION="${1:-v1.0.0}"
OUTPUT_DIR="${2:-$DIST_DIR}"

log "Generating provenance for version: $VERSION"

# Check if bundles exist
if [[ ! -f "$OUTPUT_DIR/manifest.json" ]]; then
    echo "Error: No manifest found. Run package-wit.sh first."
    exit 1
fi

# Get git information
if git rev-parse --git-dir > /dev/null 2>&1; then
    if git rev-parse HEAD > /dev/null 2>&1; then
        GIT_COMMIT=$(git rev-parse HEAD)
    else
        GIT_COMMIT="unknown"
    fi
    GIT_REPO=$(git config --get remote.origin.url 2>/dev/null || echo "unknown")
else
    GIT_COMMIT="unknown"
    GIT_REPO="unknown"
fi

# Get builder information
BUILDER_ID="github.com/Workflows/release-wit-bundles@v1"
BUILD_TYPE="https://slsa.dev/provenance/v1.0"

# Create SLSA provenance v1.0
log "Creating SLSA provenance manifest..."

cat > "$OUTPUT_DIR/provenance.json" <<EOF
{
  "_type": "$BUILD_TYPE",
  "subject": [
EOF

# Add each bundle as a subject
first=true
for tar_file in "$OUTPUT_DIR"/*.tar; do
    if [[ -f "$tar_file" ]]; then
        # Compute digest (compatible with both sha256sum and shasum)
        if command -v sha256sum &> /dev/null; then
            digest=$(sha256sum "$tar_file" | awk '{print $1}')
        else
            digest=$(shasum -a 256 "$tar_file" | awk '{print $1}')
        fi
        if [[ "$first" == "false" ]]; then
            echo "," >> "$OUTPUT_DIR/provenance.json"
        fi
        first=false
        cat >> "$OUTPUT_DIR/provenance.json" <<SUBJECT
    {
      "name": "$(basename "$tar_file")",
      "digest": {
        "sha256": "$digest"
      }
    }
SUBJECT
    fi
done

cat >> "$OUTPUT_DIR/provenance.json" <<EOF

  ],
  "predicateType": "https://slsa.dev/provenance/v1",
  "predicate": {
    "buildDefinition": {
      "buildType": "$BUILD_TYPE",
      "externalParameters": {
        "version": "$VERSION",
        "repository": "$GIT_REPO",
        "ref": "$GIT_COMMIT"
      },
      "resolvedDependencies": [
        {
          "uri": "git+${GIT_REPO}@${GIT_COMMIT}",
          "digest": {
            "gitCommit": "$GIT_COMMIT"
          }
        }
      ]
    },
    "runDetails": {
      "builder": {
        "id": "$BUILDER_ID"
      },
      "metadata": {
        "invocationId": "$(uuidgen | tr '[:upper:]' '[:lower:]')",
        "startedOn": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
        "finishedOn": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      }
    }
  }
}
EOF

log "Provenance manifest created: $OUTPUT_DIR/provenance.json"

# Create attestation bundle (unsigned)
log "Creating attestation bundle..."
cat > "$OUTPUT_DIR/attestation.json" <<EOF
{
  "payloadType": "application/vnd.in-toto+json",
  "payload": $(cat "$OUTPUT_DIR/provenance.json" | base64 | tr -d '\n' | jq -R .),
  "signatures": []
}
EOF

log "Attestation bundle created: $OUTPUT_DIR/attestation.json"
log ""
info "Note: This is an unsigned attestation bundle."
info "To sign with cosign:"
info "  cosign sign-blob --bundle attestation.bundle \\"
info "    --output-signature attestation.sig \\"
info "    --output-certificate attestation.cert \\"
info "    attestation.json"
log ""
log "Provenance generation complete!"
