//! Signing capability — produces and verifies cryptographic signatures
//! for attestation.
//!
//! `attest` binds to this trait rather than to any specific key store,
//! exactly as the audit log binds to `SecureLog` rather than to SQLite.
//! Two providers are anticipated:
//!
//! - [`SoftwareSigner`] — an in-process ed25519 key (ed25519-dalek).
//!   The default; suitable for development and for hosts without an
//!   HSM. Wasm-clean.
//!
//! - A PKCS#11-backed signer (HSM / TPM / smart card) living in a host
//!   crate, where the private key never leaves the token. It satisfies
//!   the same trait, so `attest` is unchanged.
//!
//! Verification is a free function ([`verify_ed25519`]) rather than a
//! trait method: checking a signature needs only the public key and is
//! pure math, so it never has to touch the private key or the HSM.
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use std::sync::Arc;

/// Errors a signer can return.
#[derive(Debug, Clone)]
pub enum SignerError {
    /// The signer does not implement the requested algorithm.
    UnsupportedAlgorithm(String),
    /// A key was malformed (wrong length, bad encoding, ...).
    InvalidKey(String),
    /// A signature was malformed.
    InvalidSignature(String),
    /// The backend failed (HSM error, I/O, ...).
    Backend(String),
}

impl std::fmt::Display for SignerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignerError::UnsupportedAlgorithm(a) => write!(f, "unsupported algorithm: {a}"),
            SignerError::InvalidKey(e) => write!(f, "invalid key: {e}"),
            SignerError::InvalidSignature(e) => write!(f, "invalid signature: {e}"),
            SignerError::Backend(e) => write!(f, "signer backend error: {e}"),
        }
    }
}

impl std::error::Error for SignerError {}

/// A signing backend. Produces signatures over arbitrary messages and
/// exposes the corresponding public key.
pub trait Signer: Send + Sync {
    /// Algorithm identifier, e.g. `"ed25519"`. Recorded alongside the
    /// signature so verifiers can pick the right scheme.
    fn algorithm(&self) -> &str;

    /// Sign `message`, returning the raw signature bytes.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SignerError>;

    /// The public key corresponding to this signer's private key.
    fn public_key(&self) -> Vec<u8>;
}

/// Shared, thread-safe handle to a signer.
pub type SharedSigner = Arc<dyn Signer>;

/// Algorithm string for ed25519 signers.
pub const ALG_ED25519: &str = "ed25519";

/// In-process ed25519 signer.
///
/// Constructed from a 32-byte seed (no RNG dependency, so it links
/// cleanly on every target including wasm32-wasip2). Hosts that want a
/// random key generate 32 random bytes themselves and pass them in.
#[derive(Clone)]
pub struct SoftwareSigner {
    signing_key: SigningKey,
}

impl SoftwareSigner {
    /// Build a signer from a 32-byte ed25519 seed.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    /// Convenience: build a `SharedSigner` from a seed.
    pub fn shared(seed: [u8; 32]) -> SharedSigner {
        Arc::new(Self::from_seed(seed))
    }
}

impl Signer for SoftwareSigner {
    fn algorithm(&self) -> &str {
        ALG_ED25519
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SignerError> {
        use ed25519_dalek::Signer as _;
        Ok(self.signing_key.sign(message).to_vec())
    }

    fn public_key(&self) -> Vec<u8> {
        self.signing_key.verifying_key().to_bytes().to_vec()
    }
}

/// Verify an ed25519 signature against a public key. Pure function:
/// needs no private key and no HSM, so verifiers anywhere can call it.
pub fn verify_ed25519(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<bool, SignerError> {
    let key_bytes: [u8; 32] = public_key
        .try_into()
        .map_err(|_| SignerError::InvalidKey(format!("expected 32-byte ed25519 key, got {}", public_key.len())))?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| SignerError::InvalidKey(e.to_string()))?;
    let sig_bytes: [u8; 64] = signature
        .try_into()
        .map_err(|_| SignerError::InvalidSignature(format!("expected 64-byte ed25519 signature, got {}", signature.len())))?;
    let sig = Signature::from_bytes(&sig_bytes);
    Ok(verifying_key.verify_strict(message, &sig).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_then_verify_roundtrips() {
        let signer = SoftwareSigner::from_seed([7u8; 32]);
        let msg = b"attest this claim";
        let sig = signer.sign(msg).unwrap();
        assert_eq!(signer.algorithm(), "ed25519");
        assert!(verify_ed25519(&signer.public_key(), msg, &sig).unwrap());
    }

    #[test]
    fn tampered_message_fails_verification() {
        let signer = SoftwareSigner::from_seed([7u8; 32]);
        let sig = signer.sign(b"original").unwrap();
        assert!(!verify_ed25519(&signer.public_key(), b"tampered", &sig).unwrap());
    }

    #[test]
    fn tampered_signature_fails_verification() {
        let signer = SoftwareSigner::from_seed([7u8; 32]);
        let mut sig = signer.sign(b"msg").unwrap();
        sig[0] ^= 1;
        assert!(!verify_ed25519(&signer.public_key(), b"msg", &sig).unwrap());
    }

    #[test]
    fn distinct_seeds_produce_distinct_keys() {
        let a = SoftwareSigner::from_seed([1u8; 32]);
        let b = SoftwareSigner::from_seed([2u8; 32]);
        assert_ne!(a.public_key(), b.public_key());
        // A's signature must not verify under B's key.
        let sig = a.sign(b"x").unwrap();
        assert!(!verify_ed25519(&b.public_key(), b"x", &sig).unwrap());
    }

    #[test]
    fn malformed_key_is_rejected() {
        let signer = SoftwareSigner::from_seed([7u8; 32]);
        let sig = signer.sign(b"m").unwrap();
        assert!(matches!(
            verify_ed25519(&[0u8; 10], b"m", &sig),
            Err(SignerError::InvalidKey(_))
        ));
    }
}
