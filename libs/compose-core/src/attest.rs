/// Attestation and cryptographic signing
use crate::host::{verify_ed25519, SharedClock, SharedSigner, ALG_ED25519};
use crate::types::Digest;
use serde::{Deserialize, Serialize};

/// Attestation algorithm
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Algorithm {
    Ed25519,
    EcdsaP256,
    RsaPss,
}

/// Attestation claim
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    /// Claim type (e.g., "execution", "composition")
    pub claim_type: String,
    /// Plan digest
    pub plan_digest: Digest,
    /// Artifact digest (composed component or exec result)
    pub artifact_digest: Digest,
    /// Execution key (for exec attestations)
    pub exec_key: Option<Digest>,
    /// Timestamp when attestation was created
    pub timestamp: u64,
    /// Host identifier
    pub host_id: String,
    /// Additional claims (JSON-encoded)
    pub custom_claims: Option<String>,
}

/// Attestation proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    /// The claim being attested
    pub claim: Claim,
    /// Signature algorithm used
    pub algorithm: Algorithm,
    /// Cryptographic signature
    pub signature: Vec<u8>,
    /// Public key of signer
    pub public_key: Vec<u8>,
}

/// Verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether signature is valid
    pub valid: bool,
    /// Signer identity
    pub signer: String,
    /// Verification timestamp
    pub verified_at: u64,
    /// Error message if invalid
    pub error: Option<String>,
}

/// Attestation service.
///
/// Signing is delegated to a [`Signer`](crate::host::Signer) capability:
/// in development that's an in-process ed25519 key; in production it's a
/// PKCS#11 / HSM / TPM provider where the private key never leaves the
/// token. Verification is pure ed25519 math over the public key embedded
/// in the attestation, so it needs no signer at all.
#[derive(Clone)]
pub struct AttestationService {
    host_id: String,
    signer: SharedSigner,
    clock: SharedClock,
}

impl AttestationService {
    /// Create a new attestation service over the given signing
    /// capability and clock.
    pub fn new(host_id: String, signer: SharedSigner, clock: SharedClock) -> Self {
        Self {
            host_id,
            signer,
            clock,
        }
    }

    /// The host identifier recorded on attestations created here.
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    /// Create an attestation for a claim.
    ///
    /// Only ed25519 is implemented; other algorithms are rejected
    /// rather than silently downgraded. The signature is produced by
    /// the configured signer and the signer's public key is embedded
    /// so the attestation is self-verifying.
    pub fn attest(&self, claim: Claim, algorithm: Algorithm) -> Result<Attestation, String> {
        if algorithm != Algorithm::Ed25519 {
            return Err(format!("algorithm {algorithm:?} not implemented"));
        }
        if self.signer.algorithm() != ALG_ED25519 {
            return Err(format!(
                "configured signer uses '{}', not ed25519",
                self.signer.algorithm()
            ));
        }

        let claim_bytes =
            serde_json::to_vec(&claim).map_err(|e| format!("failed to serialize claim: {e}"))?;
        let signature = self
            .signer
            .sign(&claim_bytes)
            .map_err(|e| format!("signing failed: {e}"))?;

        Ok(Attestation {
            claim,
            algorithm,
            signature,
            public_key: self.signer.public_key(),
        })
    }

    /// Verify an attestation proof against the public key it carries.
    pub fn verify(&self, attestation: &Attestation) -> Result<VerificationResult, String> {
        if attestation.algorithm != Algorithm::Ed25519 {
            return Err(format!(
                "algorithm {:?} not implemented",
                attestation.algorithm
            ));
        }

        let claim_bytes = serde_json::to_vec(&attestation.claim)
            .map_err(|e| format!("failed to serialize claim: {e}"))?;
        let valid = verify_ed25519(
            &attestation.public_key,
            &claim_bytes,
            &attestation.signature,
        )
        .map_err(|e| format!("verification failed: {e}"))?;

        Ok(VerificationResult {
            valid,
            signer: attestation.claim.host_id.clone(),
            verified_at: self.clock.now_unix_millis(),
            error: if valid {
                None
            } else {
                Some("invalid signature".to_string())
            },
        })
    }

    /// Get the host's public key for the given algorithm.
    pub fn get_public_key(&self, algorithm: Algorithm) -> Result<Vec<u8>, String> {
        if algorithm != Algorithm::Ed25519 {
            return Err(format!("algorithm {algorithm:?} not implemented"));
        }
        Ok(self.signer.public_key())
    }

    /// Export attestation to JSON format
    pub fn export(&self, attestation: &Attestation, format: &str) -> Result<String, String> {
        match format {
            "json" => serde_json::to_string_pretty(attestation)
                .map_err(|e| format!("Failed to serialize to JSON: {}", e)),
            "slsa" => {
                // Simplified SLSA format
                let slsa = serde_json::json!({
                    "_type": "https://in-toto.io/Statement/v0.1",
                    "predicateType": "https://slsa.dev/provenance/v0.2",
                    "subject": [{
                        "name": &attestation.claim.claim_type,
                        "digest": {
                            "sha256": hex::encode(&attestation.claim.artifact_digest)
                        }
                    }],
                    "predicate": {
                        "builder": {
                            "id": &attestation.claim.host_id
                        },
                        "metadata": {
                            "buildStartedOn": attestation.claim.timestamp,
                        }
                    }
                });
                Ok(slsa.to_string())
            }
            _ => Err(format!("Unsupported format: {}", format)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Clock, SoftwareSigner, SystemClock};

    fn current_timestamp() -> u64 {
        SystemClock.now_unix_millis()
    }

    fn test_service() -> AttestationService {
        AttestationService::new(
            "test-host".to_string(),
            SoftwareSigner::shared([42u8; 32]),
            SystemClock::shared(),
        )
    }

    #[test]
    fn test_attest_and_verify() {
        let service = test_service();

        let claim = Claim {
            claim_type: "execution".to_string(),
            plan_digest: vec![1, 2, 3],
            artifact_digest: vec![4, 5, 6],
            exec_key: Some(vec![7, 8, 9]),
            timestamp: current_timestamp(),
            host_id: "test-host".to_string(),
            custom_claims: None,
        };

        let attestation = service.attest(claim, Algorithm::Ed25519).unwrap();
        assert_eq!(attestation.algorithm, Algorithm::Ed25519);

        let result = service.verify(&attestation).unwrap();
        assert!(result.valid);
        assert_eq!(result.signer, "test-host");
    }

    #[test]
    fn test_invalid_signature() {
        let service = test_service();

        let claim = Claim {
            claim_type: "execution".to_string(),
            plan_digest: vec![1, 2, 3],
            artifact_digest: vec![4, 5, 6],
            exec_key: None,
            timestamp: current_timestamp(),
            host_id: "test-host".to_string(),
            custom_claims: None,
        };

        let mut attestation = service.attest(claim, Algorithm::Ed25519).unwrap();

        // Tamper with signature
        attestation.signature[0] ^= 1;

        let result = service.verify(&attestation).unwrap();
        assert!(!result.valid);
    }

    #[test]
    fn test_export_json() {
        let service = test_service();

        let claim = Claim {
            claim_type: "execution".to_string(),
            plan_digest: vec![1, 2, 3],
            artifact_digest: vec![4, 5, 6],
            exec_key: None,
            timestamp: current_timestamp(),
            host_id: "test-host".to_string(),
            custom_claims: None,
        };

        let attestation = service.attest(claim, Algorithm::Ed25519).unwrap();
        let json = service.export(&attestation, "json").unwrap();
        assert!(json.contains("execution"));
    }

    #[test]
    fn test_export_slsa() {
        let service = test_service();

        let claim = Claim {
            claim_type: "execution".to_string(),
            plan_digest: vec![1, 2, 3],
            artifact_digest: vec![4, 5, 6],
            exec_key: None,
            timestamp: current_timestamp(),
            host_id: "test-host".to_string(),
            custom_claims: None,
        };

        let attestation = service.attest(claim, Algorithm::Ed25519).unwrap();
        let slsa = service.export(&attestation, "slsa").unwrap();
        assert!(slsa.contains("in-toto.io"));
        assert!(slsa.contains("slsa.dev"));
    }
}
