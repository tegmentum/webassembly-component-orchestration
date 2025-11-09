/// Attestation and cryptographic signing
use crate::types::Digest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

/// Simple in-memory key store for dev/testing
/// In production, this would interface with HSM or KMS
struct KeyStore {
    keys: HashMap<Algorithm, (Vec<u8>, Vec<u8>)>, // (private_key, public_key)
}

impl KeyStore {
    fn new() -> Self {
        let mut keys = HashMap::new();

        // Generate dev Ed25519 keypair (deterministic for demos)
        // In production, use proper key generation and storage
        let private_key = vec![42u8; 32]; // Placeholder
        let public_key = vec![1u8; 32];  // Placeholder
        keys.insert(Algorithm::Ed25519, (private_key, public_key));

        Self { keys }
    }

    fn get_public_key(&self, algorithm: &Algorithm) -> Option<Vec<u8>> {
        self.keys.get(algorithm).map(|(_, pk)| pk.clone())
    }

    fn get_private_key(&self, algorithm: &Algorithm) -> Option<Vec<u8>> {
        self.keys.get(algorithm).map(|(sk, _)| sk.clone())
    }
}

/// Attestation service
#[derive(Clone)]
pub struct AttestationService {
    host_id: String,
    key_store: Arc<Mutex<KeyStore>>,
}

impl AttestationService {
    /// Create a new attestation service
    pub fn new(host_id: String) -> Self {
        Self {
            host_id,
            key_store: Arc::new(Mutex::new(KeyStore::new())),
        }
    }

    /// Create an attestation for a claim
    pub fn attest(&self, claim: Claim, algorithm: Algorithm) -> Result<Attestation, String> {
        let key_store = self.key_store.lock().unwrap();

        let private_key = key_store
            .get_private_key(&algorithm)
            .ok_or_else(|| format!("No key found for algorithm {:?}", algorithm))?;

        let public_key = key_store
            .get_public_key(&algorithm)
            .ok_or_else(|| format!("No public key found for algorithm {:?}", algorithm))?;

        // Serialize claim for signing
        let claim_bytes = serde_json::to_vec(&claim)
            .map_err(|e| format!("Failed to serialize claim: {}", e))?;

        // Sign the claim (simplified signature for demo)
        // In production, use proper crypto library (e.g., ed25519-dalek, ring)
        let signature = self.sign_bytes(&claim_bytes, &private_key, &algorithm)?;

        Ok(Attestation {
            claim,
            algorithm,
            signature,
            public_key,
        })
    }

    /// Verify an attestation proof
    pub fn verify(&self, attestation: &Attestation) -> Result<VerificationResult, String> {
        // Serialize claim
        let claim_bytes = serde_json::to_vec(&attestation.claim)
            .map_err(|e| format!("Failed to serialize claim: {}", e))?;

        // Verify signature
        let valid = self.verify_signature(
            &claim_bytes,
            &attestation.signature,
            &attestation.public_key,
            &attestation.algorithm,
        )?;

        Ok(VerificationResult {
            valid,
            signer: attestation.claim.host_id.clone(),
            verified_at: current_timestamp(),
            error: if valid {
                None
            } else {
                Some("Invalid signature".to_string())
            },
        })
    }

    /// Get host public key
    pub fn get_public_key(&self, algorithm: Algorithm) -> Result<Vec<u8>, String> {
        let key_store = self.key_store.lock().unwrap();
        key_store
            .get_public_key(&algorithm)
            .ok_or_else(|| format!("No public key found for algorithm {:?}", algorithm))
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

    // Simplified signing (for demo purposes)
    // In production, use proper cryptographic libraries
    fn sign_bytes(
        &self,
        data: &[u8],
        _private_key: &[u8],
        algorithm: &Algorithm,
    ) -> Result<Vec<u8>, String> {
        match algorithm {
            Algorithm::Ed25519 => {
                // Simplified signature = SHA256(data) for demo
                use sha2::{Digest as Sha2Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(data);
                Ok(hasher.finalize().to_vec())
            }
            _ => Err(format!("Algorithm {:?} not implemented", algorithm)),
        }
    }

    fn verify_signature(
        &self,
        data: &[u8],
        signature: &[u8],
        _public_key: &[u8],
        algorithm: &Algorithm,
    ) -> Result<bool, String> {
        match algorithm {
            Algorithm::Ed25519 => {
                // Simplified verification = recompute and compare
                use sha2::{Digest as Sha2Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(data);
                let computed = hasher.finalize().to_vec();
                Ok(computed == signature)
            }
            _ => Err(format!("Algorithm {:?} not implemented", algorithm)),
        }
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attest_and_verify() {
        let service = AttestationService::new("test-host".to_string());

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
        let service = AttestationService::new("test-host".to_string());

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
        let service = AttestationService::new("test-host".to_string());

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
        let service = AttestationService::new("test-host".to_string());

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
