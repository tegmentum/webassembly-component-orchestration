# Trust Verification Specification

**Version:** 1.0.0
**Package:** `sys:compose@1.0.0`

This document defines trust verification mechanisms for the Compositional WebAssembly System.

---

## Overview

Trust verification ensures that components and artifacts are from trusted sources and have not been tampered with. The system supports multiple verification backends including SigStore, PGP, and X.509.

---

## Verification Backends

### 1. SigStore

**URI Scheme:** `sigstore://`

SigStore provides keyless signing using transparency logs and OIDC identity verification.

**Configuration:**
```json
{
  "backend": "sigstore://",
  "fulcio_url": "https://fulcio.sigstore.dev",
  "rekor_url": "https://rekor.sigstore.dev",
  "identity": "user@example.com"
}
```

**Verification Process:**
1. Verify signature against Fulcio certificate
2. Check certificate was issued to expected identity
3. Verify inclusion in Rekor transparency log
4. Check timestamp is within validity period

### 2. PGP/GPG

**URI Scheme:** `pgp://` or `gpg://`

Traditional PGP signature verification.

**Configuration:**
```json
{
  "backend": "pgp://",
  "keyring": "/path/to/keyring.gpg",
  "required_keys": ["FINGERPRINT1", "FINGERPRINT2"]
}
```

### 3. X.509 Certificates

**URI Scheme:** `x509://`

Certificate-based verification with PKI.

**Configuration:**
```json
{
  "backend": "x509://",
  "ca_bundle": "/path/to/ca-bundle.pem",
  "cert_path": "/path/to/cert.pem"
}
```

### 4. Development/Local (No Verification)

**URI Scheme:** `dev://`

For development only - trusts all artifacts.

---

## Signature Formats

### Detached Signatures

Signatures are stored separately from artifacts and referenced by digest.

**Storage:**
```
.compose/trust/<digest-prefix>/<digest>.sig
```

### Embedded Metadata

Components may include embedded signature metadata in custom sections.

---

## Verification Cache

Verified artifacts are cached to avoid repeated verification:

```
Cache Key: H(digest + backend + policy)
Cache Entry: { verified: bool, metadata: VerificationMetadata, timestamp: u64 }
```

**Cache Invalidation:**
- After 24 hours (configurable)
- When trust policy changes
- Manual cache clear

---

## Trust Policies

### Strict Mode

All artifacts must be signed and verified before use.

```rust
Policy {
    trust_required: true,
    allowed_backends: ["sigstore://", "pgp://"],
    cache_ttl: 86400,
}
```

### Audit Mode

Artifacts are verified but failures are logged, not enforced.

```rust
Policy {
    trust_required: false,
    audit_failures: true,
    allowed_backends: ["sigstore://", "pgp://", "dev://"],
}
```

### Development Mode

No verification required.

```rust
Policy {
    trust_required: false,
    allowed_backends: ["dev://"],
}
```

---

## Verification Flow

### During Emit

1. Load component bytes from blob store
2. Check verification cache
3. If not cached or expired:
   - Load signature from trust store
   - Verify using configured backend
   - Update cache with result
4. If verification fails and strict mode:
   - Abort composition
5. If verification fails and audit mode:
   - Log warning
   - Continue composition

### During Exec

1. Check composed artifact digest
2. Verify against trust store
3. Check all component digests were verified during emit

---

## API Integration

### Trust Store Methods

```rust
impl TrustStore {
    /// Verify artifact with signature
    fn verify(&self, digest: &Digest, bytes: &[u8], signature: Option<&[u8]>)
        -> Result<VerificationResult>;

    /// Verify using cached result
    fn verify_digest(&self, digest: &Digest) -> Result<VerificationResult>;

    /// Add to trusted set
    fn trust_digest(&self, digest: &Digest, metadata: VerificationMetadata)
        -> Result<()>;

    /// Check if trusted
    fn is_trusted(&self, digest: &Digest) -> bool;
}
```

---

## Error Handling

| Error Code | Description | Action |
|------------|-------------|--------|
| `Trust.VerificationFailed` | Signature verification failed | Abort in strict mode |
| `Trust.SignatureInvalid` | Signature format invalid | Abort in strict mode |
| `Trust.CertificateExpired` | Certificate has expired | Abort or warn |
| `Trust.UntrustedSource` | Source not in trusted set | Abort in strict mode |
| `Trust.BackendError` | Backend operation failed | Retry or fallback |

---

## Example Usage

### Signing an Artifact

```bash
# Using SigStore
cosign sign-blob --bundle=signature.json artifact.wasm

# Using GPG
gpg --detach-sign --armor artifact.wasm
```

### Storing Signature

```bash
# Store in trust directory
composectl trust add artifact-digest \
  --signature=signature.json \
  --backend=sigstore:// \
  --identity=user@example.com
```

### Verifying During Composition

```bash
# Strict mode - verification required
composectl emit build plan.cbor \
  --trust-mode=strict \
  --trust-backend=sigstore://
```

---

**Last Updated:** 2025-01-09
