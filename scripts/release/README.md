# Release Scripts

Automation scripts for creating and distributing WIT bundle releases.

## Overview

This directory contains scripts for:
- Packaging WIT interfaces into OCI-compatible bundles
- Generating SLSA provenance manifests
- Pushing bundles to OCI registries
- Complete release automation

## Scripts

### `release.sh` - Complete Release Automation

Main entry point for creating a release.

```bash
./scripts/release/release.sh v1.0.0 [registry.example.com/wit]
```

**Steps:**
1. Packages all WIT bundles
2. Generates SLSA provenance
3. Creates git tag (if in repo)
4. Pushes to OCI registry (if provided)

**Arguments:**
- `version` - Release version tag (e.g., `v1.0.0`)
- `registry` - Optional OCI registry URL

### `package-wit.sh` - Bundle WIT Interfaces

Packages WIT directories into reproducible tar archives.

```bash
./scripts/release/package-wit.sh [version] [output-dir]
```

**Features:**
- Creates normalized tar archives for reproducibility
- Computes SHA-256 digests
- Generates metadata files
- Creates bundle manifest

**Output:**
- `target/dist/*.tar` - WIT bundle archives
- `target/dist/*.tar.metadata.json` - Bundle metadata
- `target/dist/manifest.json` - Complete manifest

**Example Output:**
```
target/dist/
├── std-secrets-v1.0.0.tar
├── std-secrets-v1.0.0.tar.metadata.json
├── std-attest-v1.0.0.tar
├── std-attest-v1.0.0.tar.metadata.json
├── sys-compose-v1.0.0.tar
├── sys-compose-v1.0.0.tar.metadata.json
└── manifest.json
```

### `generate-provenance.sh` - Create SLSA Provenance

Generates SLSA v1.0 provenance manifests for bundle attestation.

```bash
./scripts/release/generate-provenance.sh [version] [output-dir]
```

**Features:**
- Creates SLSA provenance JSON
- Records git commit and repository info
- Generates unsigned attestation bundle
- Compatible with cosign/sigstore

**Output:**
- `target/dist/provenance.json` - SLSA provenance
- `target/dist/attestation.json` - Attestation bundle

### `push-oci.sh` - Push to OCI Registry

Pushes WIT bundles to an OCI registry using oras.

```bash
./scripts/release/push-oci.sh <version> <registry>
```

**Requirements:**
- [oras CLI](https://oras.land/) installed
- Registry credentials (via `OCI_USERNAME` and `OCI_PASSWORD`)

**Environment Variables:**
- `OCI_USERNAME` - Registry username
- `OCI_PASSWORD` - Registry password
- `DRY_RUN=1` - Test mode (don't actually push)

**Example:**
```bash
export OCI_USERNAME=myuser
export OCI_PASSWORD=mypass
./scripts/release/push-oci.sh v1.0.0 registry.example.com/wit
```

## Quick Start

### Local Release (No Registry Push)

```bash
# Package and generate provenance only
./scripts/release/release.sh v1.0.0

# Review artifacts
ls -lh target/dist/
cat target/dist/manifest.json | jq .
```

### Full Release with OCI Push

```bash
# Set registry credentials
export OCI_USERNAME=myuser
export OCI_PASSWORD=mypass

# Run full release
./scripts/release/release.sh v1.0.0 registry.example.com/wit

# Push git tag
git push origin v1.0.0
```

### Dry Run

```bash
# Test OCI push without actually pushing
DRY_RUN=1 ./scripts/release/push-oci.sh v1.0.0 registry.example.com/wit
```

## Bundle Format

WIT bundles use the media type: `application/vnd.wit.bundle.v1+tar`

Each bundle is a tar archive containing:
- WIT interface files (`*.wit`)
- Package manifests (`package.wit`)
- Associated metadata

Bundles are created with reproducible builds:
- Normalized file order (sorted by name)
- Fixed timestamps (2024-01-01)
- Consistent ownership (root:root)

## Provenance Format

Provenance follows [SLSA v1.0 specification](https://slsa.dev/provenance/v1):

```json
{
  "_type": "https://slsa.dev/provenance/v1.0",
  "subject": [...],
  "predicateType": "https://slsa.dev/provenance/v1",
  "predicate": {
    "buildDefinition": {
      "buildType": "https://slsa.dev/provenance/v1.0",
      "externalParameters": {
        "version": "v1.0.0",
        "repository": "...",
        "ref": "..."
      }
    },
    "runDetails": {
      "builder": {
        "id": "github.com/Workflows/release-wit-bundles@v1"
      }
    }
  }
}
```

## Signing with Cosign

To cryptographically sign attestation bundles:

```bash
# Install cosign
# https://docs.sigstore.dev/cosign/installation/

# Sign attestation
cosign sign-blob \
  --bundle target/dist/attestation.bundle \
  --output-signature target/dist/attestation.sig \
  --output-certificate target/dist/attestation.cert \
  target/dist/attestation.json

# Verify signature
cosign verify-blob \
  --bundle target/dist/attestation.bundle \
  --certificate target/dist/attestation.cert \
  --certificate-identity <identity> \
  --certificate-oidc-issuer <issuer> \
  target/dist/attestation.json
```

## OCI Registry Usage

### Pulling Bundles

```bash
# Pull a specific bundle
oras pull registry.example.com/wit/std-secrets:v1.0.0

# Pull with digest
oras pull registry.example.com/wit/std-secrets@sha256:...

# List available versions
oras repo tags registry.example.com/wit/std-secrets
```

### Discovering Artifacts

```bash
# Show all artifacts for a bundle
oras discover registry.example.com/wit/std-secrets:v1.0.0

# Fetch manifest
oras manifest fetch registry.example.com/wit/std-secrets:v1.0.0
```

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Release WIT Bundles

on:
  push:
    tags:
      - 'v*'

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install oras
        run: |
          curl -LO https://github.com/oras-project/oras/releases/download/v1.1.0/oras_1.1.0_linux_amd64.tar.gz
          tar -xzf oras_*.tar.gz
          sudo mv oras /usr/local/bin/

      - name: Create release
        env:
          OCI_USERNAME: ${{ secrets.REGISTRY_USERNAME }}
          OCI_PASSWORD: ${{ secrets.REGISTRY_PASSWORD }}
        run: |
          ./scripts/release/release.sh ${{ github.ref_name }} registry.example.com/wit

      - name: Upload artifacts
        uses: actions/upload-artifact@v3
        with:
          name: wit-bundles
          path: target/dist/
```

## Acceptance Criteria (M9)

- ✅ Git signed tag `v1.0.0` can be created
- ✅ OCI bundles with media type `application/vnd.wit.bundle.v1+tar`
- ✅ SLSA provenance manifest generated
- ✅ Bundles can be pushed to OCI registry
- ✅ OCI digest verification possible

## Troubleshooting

### oras not found

Install from: https://oras.land/

```bash
# macOS
brew install oras

# Linux
curl -LO https://github.com/oras-project/oras/releases/download/v1.1.0/oras_1.1.0_linux_amd64.tar.gz
tar -xzf oras_*.tar.gz
sudo mv oras /usr/local/bin/
```

### Registry authentication failed

Ensure credentials are set:
```bash
export OCI_USERNAME=myuser
export OCI_PASSWORD=mypass
```

Or login manually:
```bash
oras login registry.example.com
```

### Bundle digest mismatch

Bundles are created with reproducible builds. If digests don't match:
1. Check git commit matches
2. Verify file timestamps are normalized
3. Ensure tar is creating archives consistently

## References

- [OCI Artifacts](https://github.com/opencontainers/artifacts)
- [ORAS Project](https://oras.land/)
- [SLSA Provenance](https://slsa.dev/provenance/)
- [Cosign](https://docs.sigstore.dev/cosign/overview/)
- [WIT Format](https://component-model.bytecodealliance.org/design/wit.html)
