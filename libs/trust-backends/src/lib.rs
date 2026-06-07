//! Native trust backends for the compose orchestrator.
//!
//! `compose-core` is wasm32-wasip2 clean and intentionally ships only the
//! `dev` trust backend plus the [`TrustBackend`] trait. The real backends
//! (SigStore, PGP) need heavy, often-native dependencies, so they live in
//! this native crate and are registered by the wasmtime host at startup
//! via `TrustStore::register_backend`.
//!
//! Status: both backends perform real verification. PGP checks detached
//! OpenPGP signatures (via rPGP); SigStore does offline Sigstore-bundle
//! verification (via `sigstore-verify`) — signature, Fulcio cert chain,
//! Rekor inclusion proof, and SCT against a trusted root.
use compose_core::blobs::compute_digest;
use compose_core::host::SharedClock;
use compose_core::trust::TrustBackend;
use compose_core::types::{Digest, Error, ErrorCode, VerificationMetadata};
use pgp::composed::Deserializable;
use std::path::PathBuf;

/// A required signing identity: the certificate SAN identity and the OIDC
/// issuer that must have minted it. Verification accepts an artifact only if
/// one configured identity matches the bundle's certificate.
#[derive(Debug, Clone)]
pub struct SigstoreIdentity {
    /// Expected certificate SAN identity (an email or URI).
    pub identity: String,
    /// Expected OIDC issuer (e.g. `https://token.actions.githubusercontent.com`).
    pub issuer: String,
}

/// SigStore trust backend: offline verification of a Sigstore bundle.
///
/// The `signature` argument to [`TrustBackend::verify`] is a Sigstore
/// **bundle** (`*.sigstore.json`, v0.1–0.3). Verification is fully offline —
/// the bundle carries the Fulcio signing certificate, the artifact signature,
/// and the Rekor inclusion proof + signed entry timestamp. We check, via
/// `sigstore-verify`: the signature over the artifact, the cert chain to the
/// trusted root's Fulcio CA, the certificate's SCT, and the transparency-log
/// inclusion proof. The trusted root defaults to the embedded Sigstore
/// production root; a custom root JSON may be supplied.
pub struct SigStoreTrustBackend {
    trusted_root: sigstore_verify::trust_root::TrustedRoot,
    identities: Vec<SigstoreIdentity>,
    clock: SharedClock,
}

impl SigStoreTrustBackend {
    /// Create a backend trusting the embedded Sigstore **production** root,
    /// with no identity restriction (any valid Fulcio identity is accepted).
    pub fn new(clock: SharedClock) -> Self {
        Self::production(Vec::new(), clock)
            .expect("embedded Sigstore production trusted root must parse")
    }

    /// Create a backend trusting the embedded Sigstore production root,
    /// restricted to the given signing identities (empty = no restriction).
    pub fn production(
        identities: Vec<SigstoreIdentity>,
        clock: SharedClock,
    ) -> Result<Self, Error> {
        let trusted_root = sigstore_verify::trust_root::TrustedRoot::from_json(
            sigstore_verify::trust_root::SIGSTORE_PRODUCTION_TRUSTED_ROOT,
        )
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("invalid embedded Sigstore trusted root: {e:?}"),
            )
        })?;
        Ok(Self {
            trusted_root,
            identities,
            clock,
        })
    }

    /// Create a backend with a custom trusted-root JSON file (e.g. a private
    /// Sigstore instance). When `trust_root_path` is `None`, falls back to the
    /// embedded production root.
    pub fn with_trust_root(
        trust_root_path: Option<std::path::PathBuf>,
        identities: Vec<SigstoreIdentity>,
        clock: SharedClock,
    ) -> Result<Self, Error> {
        let Some(path) = trust_root_path else {
            return Self::production(identities, clock);
        };
        let json = std::fs::read_to_string(&path).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to read trusted root {}: {e}", path.display()),
            )
        })?;
        let trusted_root =
            sigstore_verify::trust_root::TrustedRoot::from_json(&json).map_err(|e| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("invalid trusted root {}: {e:?}", path.display()),
                )
            })?;
        Ok(Self {
            trusted_root,
            identities,
            clock,
        })
    }

    /// The verification policies to try: one per configured identity, or a
    /// single unrestricted policy when none are configured. All policies keep
    /// the full set of checks (cert chain, SCT, transparency-log inclusion).
    fn policies(&self) -> Vec<sigstore_verify::VerificationPolicy> {
        if self.identities.is_empty() {
            return vec![sigstore_verify::VerificationPolicy::default()];
        }
        self.identities
            .iter()
            .map(|id| {
                sigstore_verify::VerificationPolicy::default()
                    .require_identity(id.identity.clone())
                    .require_issuer(id.issuer.clone())
            })
            .collect()
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
        // The artifact bytes must match the digest we were asked to trust.
        let computed = compute_digest(bytes);
        if &computed != digest {
            return Err(Error::new(
                ErrorCode::TrustVerificationFailed,
                "digest mismatch",
            ));
        }

        // The signature payload is a Sigstore bundle (JSON).
        let bundle_json = std::str::from_utf8(signature).map_err(|_| {
            Error::new(
                ErrorCode::TrustSignatureInvalid,
                "sigstore bundle is not valid UTF-8 JSON",
            )
        })?;
        let bundle = sigstore_verify::types::Bundle::from_json(bundle_json).map_err(|e| {
            Error::new(
                ErrorCode::TrustSignatureInvalid,
                format!("invalid sigstore bundle: {e:?}"),
            )
        })?;

        // Try each policy (identity); accept on the first that verifies.
        let mut last_err: Option<String> = None;
        for policy in self.policies() {
            match sigstore_verify::verify(bytes, &bundle, &policy, &self.trusted_root) {
                Ok(result) if result.success => {
                    let signer = result
                        .identity
                        .or(result.issuer)
                        .unwrap_or_else(|| "unknown".to_string());
                    let timestamp = result
                        .integrated_time
                        .and_then(|t| u64::try_from(t).ok())
                        .or_else(|| Some(self.clock.now_unix_secs()));
                    return Ok(VerificationMetadata {
                        signer,
                        timestamp,
                        backend: "sigstore".to_string(),
                    });
                }
                Ok(result) => {
                    last_err = Some(format!(
                        "verification did not succeed (warnings: {:?})",
                        result.warnings
                    ));
                }
                Err(e) => last_err = Some(format!("{e:?}")),
            }
        }

        Err(Error::new(
            ErrorCode::TrustSignatureInvalid,
            format!(
                "sigstore bundle did not verify: {}",
                last_err.unwrap_or_else(|| "no matching identity".to_string())
            ),
        ))
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
