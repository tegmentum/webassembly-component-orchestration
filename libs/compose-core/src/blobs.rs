/// File-backed content-addressed storage (CAS) for blobs.
///
/// When compose-core compiles to wasm32-wasip2 these std::fs calls are
/// lowered to wasi:filesystem imports by libstd, so the same code runs
/// whether the orchestrator is the embedded Rust library or a wasm
/// component.
use crate::types::{Digest, Error, ErrorCode};
use anyhow::Result;
use sha2::{Digest as Sha2Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use walkdir::WalkDir;

/// Content-addressed blob storage backend.
///
/// The single storage trait every CAS backend in the system implements.
/// Addressing is SHA-256 (`compute_digest`) — the Phase-1 `content_digest`.
/// Two backends implement this trait:
///
/// * [`FsBlobStore`] — the file-system store in this crate (used by the
///   framework and the ducklink host).
/// * `SqliteCasStore` — sqlink's SQLite-backed store (in the `sqlite-cas-cache`
///   crate, backed by `sqlite-component-core` / sqlite-wasm).
///
/// Keeping the trait here lets `CompileCache<B: BlobBackend>` (also in this
/// crate) stay generic over the backend while compose-core remains
/// wasmtime-free and sqlite-free.
pub trait BlobBackend {
    /// Store `bytes` and return their SHA-256 digest. Idempotent: storing the
    /// same bytes twice yields the same digest and is not an error.
    fn put(&self, bytes: &[u8]) -> Result<Digest, Error>;

    /// Retrieve a blob by digest, verifying the stored bytes re-hash to
    /// `digest` (returns [`ErrorCode::BlobDigestMismatch`] on corruption,
    /// [`ErrorCode::BlobNotFound`] when absent).
    fn get(&self, digest: &Digest) -> Result<Vec<u8>, Error>;

    /// Whether a blob with this digest is present.
    fn has(&self, digest: &Digest) -> bool;

    /// Size in bytes of a stored blob, if present.
    fn size(&self, digest: &Digest) -> Option<u64>;

    /// Delete a blob by digest.
    fn delete(&self, digest: &Digest) -> Result<(), Error>;

    /// List the digests of every stored blob.
    fn list_all(&self) -> Vec<Digest>;
}

/// Content-addressed blob storage using the file system.
///
/// The framework's reference [`BlobBackend`]. Historically named `BlobStore`;
/// that name is preserved as a type alias for back-compat.
#[derive(Debug, Clone)]
pub struct FsBlobStore {
    root: PathBuf,
    max_size: u64,
}

/// Back-compat alias for [`FsBlobStore`], the file-system [`BlobBackend`].
pub type BlobStore = FsBlobStore;

impl FsBlobStore {
    /// Create a new blob store at the given path
    pub fn new(root: PathBuf, max_size: u64) -> Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root, max_size })
    }

    /// Store blob and return its SHA-256 digest
    pub fn put(&self, bytes: &[u8]) -> Result<Digest, Error> {
        if bytes.len() as u64 > self.max_size {
            return Err(Error::new(
                ErrorCode::BlobIoError,
                format!(
                    "blob size {} exceeds maximum {}",
                    bytes.len(),
                    self.max_size
                ),
            ));
        }

        let digest = compute_digest(bytes);
        let path = self.digest_path(&digest);

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::new(
                    ErrorCode::BlobIoError,
                    format!("failed to create directory: {}", e),
                )
            })?;
        }

        // Write atomically using temp file
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, bytes).map_err(|e| {
            Error::new(
                ErrorCode::BlobIoError,
                format!("failed to write blob: {}", e),
            )
        })?;

        fs::rename(&temp_path, &path).map_err(|e| {
            Error::new(
                ErrorCode::BlobIoError,
                format!("failed to rename blob: {}", e),
            )
        })?;

        Ok(digest)
    }

    /// Retrieve blob by digest
    pub fn get(&self, digest: &Digest) -> Result<Vec<u8>, Error> {
        let path = self.digest_path(digest);
        let bytes = fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::new(
                    ErrorCode::BlobNotFound,
                    format!("blob {} not found", hex::encode(digest)),
                )
            } else {
                Error::new(
                    ErrorCode::BlobIoError,
                    format!("failed to read blob: {}", e),
                )
            }
        })?;

        // Verify digest matches
        let computed = compute_digest(&bytes);
        if &computed != digest {
            return Err(Error::new(
                ErrorCode::BlobDigestMismatch,
                format!(
                    "digest mismatch: expected {}, got {}",
                    hex::encode(digest),
                    hex::encode(&computed)
                ),
            ));
        }

        Ok(bytes)
    }

    /// Check if blob exists
    pub fn has(&self, digest: &Digest) -> bool {
        self.digest_path(digest).exists()
    }

    /// Get blob size without retrieving content
    pub fn size(&self, digest: &Digest) -> Option<u64> {
        let path = self.digest_path(digest);
        fs::metadata(path).ok().map(|m| m.len())
    }

    /// Delete blob by digest
    pub fn delete(&self, digest: &Digest) -> Result<(), Error> {
        let path = self.digest_path(digest);
        fs::remove_file(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::new(
                    ErrorCode::BlobNotFound,
                    format!("blob {} not found", hex::encode(digest)),
                )
            } else {
                Error::new(
                    ErrorCode::BlobIoError,
                    format!("failed to delete blob: {}", e),
                )
            }
        })
    }

    /// List all blob digests
    pub fn list_all(&self) -> Vec<Digest> {
        let mut digests = Vec::new();

        for entry in WalkDir::new(&self.root).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                // Reconstruct full digest from sharded path: root/AB/CDEF... -> ABCDEF...
                if let Some(parent) = entry.path().parent() {
                    if let Some(prefix) = parent.file_name().and_then(|p| p.to_str()) {
                        if let Some(suffix) = entry.file_name().to_str() {
                            let full_hex = format!("{}{}", prefix, suffix);
                            if let Ok(digest) = hex::decode(&full_hex) {
                                if digest.len() == 32 {
                                    // SHA-256 is 32 bytes
                                    digests.push(digest);
                                }
                            }
                        }
                    }
                }
            }
        }

        digests
    }

    /// Get the file path for a digest
    fn digest_path(&self, digest: &Digest) -> PathBuf {
        let hex_digest = hex::encode(digest);
        // Use first 2 chars as directory for sharding
        let dir = &hex_digest[..2];
        let file = &hex_digest[2..];
        self.root.join(dir).join(file)
    }
}

impl BlobBackend for FsBlobStore {
    fn put(&self, bytes: &[u8]) -> Result<Digest, Error> {
        FsBlobStore::put(self, bytes)
    }

    fn get(&self, digest: &Digest) -> Result<Vec<u8>, Error> {
        FsBlobStore::get(self, digest)
    }

    fn has(&self, digest: &Digest) -> bool {
        FsBlobStore::has(self, digest)
    }

    fn size(&self, digest: &Digest) -> Option<u64> {
        FsBlobStore::size(self, digest)
    }

    fn delete(&self, digest: &Digest) -> Result<(), Error> {
        FsBlobStore::delete(self, digest)
    }

    fn list_all(&self) -> Vec<Digest> {
        FsBlobStore::list_all(self)
    }
}

/// Compute SHA-256 digest of bytes
pub fn compute_digest(bytes: &[u8]) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

/// Compute SHA-256 digest with "witcanon:1" prefix for WIT IDs
pub fn compute_wit_digest(bytes: &[u8]) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(b"witcanon:1");
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_put_get_blob() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();

        let data = b"hello world";
        let digest = store.put(data).unwrap();

        assert!(store.has(&digest));
        let retrieved = store.get(&digest).unwrap();
        assert_eq!(data, &retrieved[..]);
    }

    #[test]
    fn test_digest_mismatch() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();

        let data = b"hello world";
        let digest = store.put(data).unwrap();

        // Corrupt the file
        let path = store.digest_path(&digest);
        fs::write(&path, b"corrupted").unwrap();

        let result = store.get(&digest);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().code,
            ErrorCode::BlobDigestMismatch
        ));
    }

    #[test]
    fn test_list_all() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();

        let data1 = b"blob1";
        let data2 = b"blob2";

        let digest1 = store.put(data1).unwrap();
        let digest2 = store.put(data2).unwrap();

        let all = store.list_all();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&digest1));
        assert!(all.contains(&digest2));
    }

    #[test]
    fn test_blob_backend_trait_object() {
        let dir = tempdir().unwrap();
        let store = FsBlobStore::new(dir.path().to_path_buf(), 1024 * 1024).unwrap();

        // Exercise FsBlobStore through the BlobBackend trait surface.
        let backend: &dyn BlobBackend = &store;
        let data = b"trait dispatch";
        let digest = backend.put(data).unwrap();
        assert!(backend.has(&digest));
        assert_eq!(backend.size(&digest), Some(data.len() as u64));
        assert_eq!(backend.get(&digest).unwrap(), data);
        assert_eq!(backend.list_all(), vec![digest.clone()]);
        backend.delete(&digest).unwrap();
        assert!(!backend.has(&digest));
    }
}
