//! Native trust backends for the compose orchestrator.
//!
//! `compose-core` is wasm32-wasip2 clean and intentionally ships only the
//! `dev` trust backend plus the [`TrustBackend`] trait. The real backends
//! (SigStore, PGP) need heavy, often-native dependencies, so they live in
//! this native crate and are registered by the wasmtime host at startup
//! via `TrustStore::register_backend`.
//!
//! Status: the PGP backend performs real detached-signature verification
//! (via rPGP). The SigStore backend is still a stub pending offline
//! bundle verification; see `docs/remaining-work-plan.md`.
use compose_core::blobs::compute_digest;
use compose_core::host::SharedClock;
use compose_core::trust::TrustBackend;
use compose_core::types::{Digest, Error, ErrorCode, VerificationMetadata};
use pgp::composed::Deserializable;
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

/// PGP trust backend: verifies a detached OpenPGP signature over the
/// artifact against a keyring of trusted public keys (via rPGP).
pub struct PgpTrustBackend {
    keyring_path: PathBuf,
    clock: SharedClock,
}

impl PgpTrustBackend {
    /// Create a new PGP backend over the given keyring (an armored public
    /// key file, which may contain multiple keys).
    pub fn new(keyring_path: PathBuf, clock: SharedClock) -> Self {
        Self {
            keyring_path,
            clock,
        }
    }

    /// Load the trusted public keys from the keyring file (ASCII-armored).
    fn load_keys(&self) -> Result<Vec<pgp::composed::SignedPublicKey>, Error> {
        let bytes = std::fs::read(&self.keyring_path).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!(
                    "failed to read keyring {}: {e}",
                    self.keyring_path.display()
                ),
            )
        })?;
        let (parsed, _headers) = pgp::composed::SignedPublicKey::from_armor_many(
            std::io::Cursor::new(bytes),
        )
        .map_err(|e| Error::new(ErrorCode::InternalError, format!("invalid keyring: {e:?}")))?;
        let keys: Vec<_> = parsed.flatten().collect();
        if keys.is_empty() {
            return Err(Error::new(
                ErrorCode::InternalError,
                "keyring contains no public keys",
            ));
        }
        Ok(keys)
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
        signature: &[u8],
    ) -> Result<VerificationMetadata, Error> {
        use pgp::composed::DetachedSignature;
        use pgp::types::KeyDetails;

        let computed = compute_digest(bytes);
        if &computed != digest {
            return Err(Error::new(
                ErrorCode::TrustVerificationFailed,
                "digest mismatch",
            ));
        }

        let keys = self.load_keys()?;

        // Accept an armored or binary detached signature.
        let sig = DetachedSignature::from_armor_single(std::io::Cursor::new(signature))
            .map(|(s, _)| s)
            .or_else(|_| DetachedSignature::from_bytes(std::io::Cursor::new(signature)))
            .map_err(|e| {
                Error::new(
                    ErrorCode::TrustSignatureInvalid,
                    format!("invalid detached signature: {e:?}"),
                )
            })?;

        // Accept if any trusted key (primary or a subkey) verifies it.
        for key in &keys {
            let ok = sig.verify(&key.primary_key, bytes).is_ok()
                || key
                    .public_subkeys
                    .iter()
                    .any(|sub| sig.verify(sub, bytes).is_ok());
            if ok {
                let signer = key
                    .details
                    .users
                    .first()
                    .map(|u| String::from_utf8_lossy(u.id.id()).into_owned())
                    .unwrap_or_else(|| hex::encode(key.fingerprint().as_bytes()));
                return Ok(VerificationMetadata {
                    signer,
                    timestamp: Some(self.clock.now_unix_secs()),
                    backend: "pgp".to_string(),
                });
            }
        }

        Err(Error::new(
            ErrorCode::TrustSignatureInvalid,
            "no key in the keyring verified the signature",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compose_core::blobs::compute_digest;
    use compose_core::host::SystemClock;
    use pgp::composed::{
        DetachedSignature, KeyType, SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey,
    };
    use pgp::crypto::hash::HashAlgorithm;
    use pgp::types::Password;

    /// Generate a fresh Ed25519 signing key (fast, unlike RSA).
    fn gen_key() -> SignedSecretKey {
        SecretKeyParamsBuilder::default()
            .key_type(KeyType::Ed25519)
            .can_sign(true)
            .can_certify(true)
            .primary_user_id("Test Signer <test@example.com>".into())
            .build()
            .expect("params")
            .generate(rand::rngs::OsRng)
            .expect("generate")
    }

    /// Write a key's armored public half to a temp keyring file.
    fn write_keyring(secret: &SignedSecretKey) -> (tempfile::TempDir, std::path::PathBuf) {
        let public = SignedPublicKey::from(secret.clone());
        let armored = public
            .to_armored_bytes(None.into())
            .expect("armor public key");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keyring.asc");
        std::fs::write(&path, armored).expect("write keyring");
        (dir, path)
    }

    fn sign(secret: &SignedSecretKey, data: &[u8]) -> Vec<u8> {
        DetachedSignature::sign_binary_data(
            rand::rngs::OsRng,
            &secret.primary_key,
            &Password::empty(),
            HashAlgorithm::Sha256,
            data,
        )
        .expect("sign")
        .to_armored_bytes(None.into())
        .expect("armor sig")
    }

    #[test]
    fn verifies_a_valid_detached_signature() {
        let secret = gen_key();
        let (_dir, keyring) = write_keyring(&secret);
        let data = b"artifact bytes to trust";
        let sig = sign(&secret, data);

        let backend = PgpTrustBackend::new(keyring, SystemClock::shared());
        let meta = backend
            .verify(&compute_digest(data), data, &sig)
            .expect("valid signature must verify");
        assert_eq!(meta.backend, "pgp");
        assert!(
            meta.signer.contains("test@example.com"),
            "signer: {}",
            meta.signer
        );
    }

    #[test]
    fn rejects_signature_from_an_untrusted_key() {
        let signer_key = gen_key();
        let other_key = gen_key();
        // Keyring holds only `other_key`, but the artifact is signed by `signer_key`.
        let (_dir, keyring) = write_keyring(&other_key);
        let data = b"artifact bytes";
        let sig = sign(&signer_key, data);

        let backend = PgpTrustBackend::new(keyring, SystemClock::shared());
        let err = backend
            .verify(&compute_digest(data), data, &sig)
            .expect_err("untrusted signer must be rejected");
        assert!(matches!(err.code, ErrorCode::TrustSignatureInvalid));
    }

    #[test]
    fn rejects_tampered_artifact() {
        let secret = gen_key();
        let (_dir, keyring) = write_keyring(&secret);
        let data = b"original";
        let sig = sign(&secret, data);
        let tampered = b"tampered";

        let backend = PgpTrustBackend::new(keyring, SystemClock::shared());
        // digest precondition catches the mismatch first.
        let err = backend
            .verify(&compute_digest(tampered), tampered, &sig)
            .expect_err("tampered artifact must be rejected");
        assert!(matches!(
            err.code,
            ErrorCode::TrustVerificationFailed | ErrorCode::TrustSignatureInvalid
        ));
    }
}
