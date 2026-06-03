/// Trust verification and signature checking
use crate::host::SharedClock;
use crate::types::{Digest, Error, ErrorCode, VerificationMetadata, VerificationResult};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod backends;
pub use backends::*;

/// Verification cache entry
#[derive(Debug, Clone)]
struct CacheEntry {
    metadata: VerificationMetadata,
    cached_at: u64,
}

/// Trust store for managing verified artifacts
#[derive(Clone)]
pub struct TrustStore {
    trust_dir: PathBuf,
    trusted: Arc<Mutex<HashMap<Vec<u8>, VerificationMetadata>>>,
    backends: Arc<Mutex<HashMap<String, Box<dyn TrustBackend>>>>,
    cache: Arc<Mutex<HashMap<Vec<u8>, CacheEntry>>>,
    cache_ttl: u64, // Cache TTL in seconds
    clock: SharedClock,
}

impl TrustStore {
    /// Create a new trust store with default cache TTL (24 hours)
    pub fn new(trust_dir: PathBuf, clock: SharedClock) -> anyhow::Result<Self> {
        Self::with_ttl(trust_dir, 86400, clock)
    }

    /// Create a new trust store with custom cache TTL
    pub fn with_ttl(
        trust_dir: PathBuf,
        cache_ttl: u64,
        clock: SharedClock,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&trust_dir)?;

        let store = Self {
            trust_dir,
            trusted: Arc::new(Mutex::new(HashMap::new())),
            backends: Arc::new(Mutex::new(HashMap::new())),
            cache: Arc::new(Mutex::new(HashMap::new())),
            cache_ttl,
            clock: clock.clone(),
        };

        // Register default backends
        store.register_backend(Box::new(DevTrustBackend::new(clock.clone())))?;
        store.register_backend(Box::new(SigStoreTrustBackend::new(clock)))?;

        Ok(store)
    }

    fn now_secs(&self) -> u64 {
        self.clock.now_unix_secs()
    }

    /// Register a trust backend
    pub fn register_backend(&self, backend: Box<dyn TrustBackend>) -> anyhow::Result<()> {
        let scheme = backend.scheme().to_string();
        self.backends.lock().unwrap().insert(scheme, backend);
        Ok(())
    }

    /// Verify artifact bytes against a digest and optional signature
    pub fn verify(
        &self,
        digest: &Digest,
        bytes: &[u8],
        signature: Option<&[u8]>,
    ) -> Result<VerificationResult, Error> {
        self.verify_with_backend(digest, bytes, signature, "dev")
    }

    /// Verify with specific backend
    pub fn verify_with_backend(
        &self,
        digest: &Digest,
        bytes: &[u8],
        signature: Option<&[u8]>,
        backend_scheme: &str,
    ) -> Result<VerificationResult, Error> {
        // Check cache first
        if let Some(cached) = self.get_cached(digest) {
            tracing::debug!("Trust verification cache hit for {}", hex::encode(digest));
            return Ok(VerificationResult {
                verified: true,
                metadata: cached.metadata,
            });
        }

        // Get backend
        let backends = self.backends.lock().unwrap();
        let backend = backends.get(backend_scheme).ok_or_else(|| {
            Error::new(
                ErrorCode::TrustVerificationFailed,
                format!("unknown trust backend: {}", backend_scheme),
            )
        })?;

        // Verify
        let signature_bytes = signature
            .ok_or_else(|| Error::new(ErrorCode::TrustVerificationFailed, "signature required"))?;

        let metadata = backend.verify(digest, bytes, signature_bytes)?;

        // Cache the result
        self.cache_verification(digest, metadata.clone());

        Ok(VerificationResult {
            verified: true,
            metadata,
        })
    }

    /// Get cached verification result
    fn get_cached(&self, digest: &Digest) -> Option<CacheEntry> {
        let cache = self.cache.lock().unwrap();
        if let Some(entry) = cache.get(digest) {
            let age = self.now_secs() - entry.cached_at;
            if age < self.cache_ttl {
                return Some(entry.clone());
            }
        }
        None
    }

    /// Cache a verification result
    fn cache_verification(&self, digest: &Digest, metadata: VerificationMetadata) {
        let entry = CacheEntry {
            metadata,
            cached_at: self.now_secs(),
        };
        self.cache.lock().unwrap().insert(digest.clone(), entry);
    }

    /// Clear verification cache
    pub fn clear_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    /// Verify artifact by digest only (lookup from trust store)
    pub fn verify_digest(&self, digest: &Digest) -> Result<VerificationResult, Error> {
        let trusted = self.trusted.lock().unwrap();

        if let Some(metadata) = trusted.get(digest) {
            Ok(VerificationResult {
                verified: true,
                metadata: metadata.clone(),
            })
        } else {
            Err(Error::new(
                ErrorCode::TrustUntrustedSource,
                format!("digest {} not in trusted set", hex::encode(digest)),
            ))
        }
    }

    /// Check if a digest is in the trusted set
    pub fn is_trusted(&self, digest: &Digest) -> bool {
        self.trusted.lock().unwrap().contains_key(digest)
    }

    /// Add a digest to the trusted set
    pub fn trust_digest(
        &self,
        digest: &Digest,
        metadata: VerificationMetadata,
    ) -> Result<(), Error> {
        self.trusted
            .lock()
            .unwrap()
            .insert(digest.clone(), metadata);

        // Persist to disk
        let trust_file = self.trust_file_path(digest);
        if let Some(parent) = trust_file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("failed to create trust directory: {}", e),
                )
            })?;
        }

        let metadata_json = serde_json::to_string_pretty(&self.trusted.lock().unwrap().get(digest))
            .map_err(|e| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("failed to serialize metadata: {}", e),
                )
            })?;

        std::fs::write(&trust_file, metadata_json).map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to write trust file: {}", e),
            )
        })?;

        Ok(())
    }

    /// Remove a digest from the trusted set
    pub fn untrust_digest(&self, digest: &Digest) -> Result<(), Error> {
        self.trusted.lock().unwrap().remove(digest);

        let trust_file = self.trust_file_path(digest);
        if trust_file.exists() {
            std::fs::remove_file(&trust_file).map_err(|e| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("failed to remove trust file: {}", e),
                )
            })?;
        }

        Ok(())
    }

    /// Get trust file path for a digest
    fn trust_file_path(&self, digest: &Digest) -> PathBuf {
        let hex_digest = hex::encode(digest);
        self.trust_dir
            .join(&hex_digest[..2])
            .join(format!("{}.json", &hex_digest[2..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::SystemClock;
    use tempfile::tempdir;

    #[test]
    fn test_trust_digest() {
        let dir = tempdir().unwrap();
        let store = TrustStore::new(dir.path().to_path_buf(), SystemClock::shared()).unwrap();

        let digest = vec![0u8; 32];
        let metadata = VerificationMetadata {
            signer: "test".to_string(),
            timestamp: Some(12345),
            backend: "test".to_string(),
        };

        store.trust_digest(&digest, metadata.clone()).unwrap();
        assert!(store.is_trusted(&digest));

        let result = store.verify_digest(&digest).unwrap();
        assert!(result.verified);
        assert_eq!(result.metadata.signer, "test");

        store.untrust_digest(&digest).unwrap();
        assert!(!store.is_trusted(&digest));
    }
}
