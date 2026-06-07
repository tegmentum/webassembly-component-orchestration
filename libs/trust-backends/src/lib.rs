//! Native trust backends for the compose orchestrator.
//!
//! `compose-core` is wasm32-wasip2 clean and intentionally ships only the
//! `dev` trust backend plus the [`TrustBackend`] trait. The real backends
//! (SigStore, PGP) need heavy, often-native dependencies, so they live in
//! this native crate and are registered by the wasmtime host at startup
//! via `TrustStore::register_backend`.
//!
//! Status: the SigStore and PGP backends are currently stubs (moved here
//! verbatim from compose-core). Real verification is implemented in
//! follow-up work; see `docs/remaining-work-plan.md`.
use compose_core::blobs::compute_digest;
use compose_core::host::SharedClock;
use compose_core::trust::TrustBackend;
use compose_core::types::{Digest, Error, ErrorCode, VerificationMetadata};
use std::path::PathBuf;

/// SigStore trust backend.
pub struct SigStoreTrustBackend {
    // Configured endpoints for a future real SigStore integration (stub today).
    #[allow(dead_code)]
    fulcio_url: String,
    #[allow(dead_code)]
    rekor_url: String,
    clock: SharedClock,
}

impl SigStoreTrustBackend {
    /// Create a new SigStore backend.
    pub fn new(clock: SharedClock) -> Self {
        Self {
            fulcio_url: "https://fulcio.sigstore.dev".to_string(),
            rekor_url: "https://rekor.sigstore.dev".to_string(),
            clock,
        }
    }

    /// Create with custom URLs.
    pub fn with_urls(fulcio_url: String, rekor_url: String, clock: SharedClock) -> Self {
        Self {
            fulcio_url,
            rekor_url,
            clock,
        }
    }
}

impl TrustBackend for SigStoreTrustBackend {
    fn scheme(&self) -> &str {
        "sigstore"
    }

    fn verify(
        &self,
        digest: &Digest,
        bytes: &[u8],
        signature: &[u8],
    ) -> Result<VerificationMetadata, Error> {
        let computed = compute_digest(bytes);
        if &computed != digest {
            return Err(Error::new(
                ErrorCode::TrustVerificationFailed,
                "digest mismatch",
            ));
        }

        // STUB. A full implementation verifies an offline Sigstore bundle:
        // signature over the artifact, cert chain to the Fulcio root, the
        // Rekor inclusion proof + SET, and an identity policy. See
        // docs/remaining-work-plan.md (Item 3).
        tracing::info!(
            digest = hex::encode(digest),
            "SigStore verification (stub) - would verify a Sigstore bundle"
        );

        let sig_str = String::from_utf8_lossy(signature);
        let identity = if sig_str.contains("identity") {
            "verified@example.com".to_string()
        } else {
            "unknown".to_string()
        };

        Ok(VerificationMetadata {
            signer: identity,
            timestamp: Some(self.clock.now_unix_secs()),
            backend: "sigstore".to_string(),
        })
    }
}

/// PGP trust backend.
pub struct PgpTrustBackend {
    #[allow(dead_code)]
    keyring_path: PathBuf,
}

impl PgpTrustBackend {
    /// Create a new PGP backend over the given keyring path.
    pub fn new(keyring_path: PathBuf) -> Self {
        Self { keyring_path }
    }
}

impl TrustBackend for PgpTrustBackend {
    fn scheme(&self) -> &str {
        "pgp"
    }

    fn verify(
        &self,
        digest: &Digest,
        bytes: &[u8],
        _signature: &[u8],
    ) -> Result<VerificationMetadata, Error> {
        let computed = compute_digest(bytes);
        if &computed != digest {
            return Err(Error::new(
                ErrorCode::TrustVerificationFailed,
                "digest mismatch",
            ));
        }

        // STUB. A full implementation loads the keyring, verifies a detached
        // OpenPGP signature over the artifact (via rPGP), and extracts the
        // signer. See docs/remaining-work-plan.md (Item 1).
        tracing::warn!("PGP verification not yet implemented");
        Err(Error::new(
            ErrorCode::NotImplemented,
            "PGP verification not yet implemented",
        ))
    }
}
