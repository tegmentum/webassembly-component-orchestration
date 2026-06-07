/// Trust verification backends
use super::*;
use crate::host::SharedClock;

/// Trust backend trait
pub trait TrustBackend: Send + Sync {
    /// Get the backend scheme (e.g., "sigstore", "pgp", "x509")
    fn scheme(&self) -> &str;

    /// Verify bytes against signature
    fn verify(
        &self,
        digest: &Digest,
        bytes: &[u8],
        signature: &[u8],
    ) -> Result<VerificationMetadata, Error>;
}

/// Development trust backend (no verification)
pub struct DevTrustBackend {
    clock: SharedClock,
}

impl DevTrustBackend {
    pub fn new(clock: SharedClock) -> Self {
        Self { clock }
    }
}

impl TrustBackend for DevTrustBackend {
    fn scheme(&self) -> &str {
        "dev"
    }

    fn verify(
        &self,
        digest: &Digest,
        bytes: &[u8],
        _signature: &[u8],
    ) -> Result<VerificationMetadata, Error> {
        // Just verify digest matches
        let computed = crate::blobs::compute_digest(bytes);
        if &computed != digest {
            return Err(Error::new(
                ErrorCode::TrustVerificationFailed,
                "digest mismatch",
            ));
        }

        Ok(VerificationMetadata {
            signer: "dev-mode".to_string(),
            timestamp: Some(self.clock.now_unix_secs()),
            backend: "dev".to_string(),
        })
    }
}

// The SigStore and PGP backends moved to the native `trust-backends` crate
// (they need heavy, often-native deps that would break this crate's
// wasm32-wasip2 build). The host registers them at startup.
