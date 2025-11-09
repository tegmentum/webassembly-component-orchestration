/// File-backed content-addressed storage (CAS) for blobs
use crate::types::{Digest, Error, ErrorCode};
use anyhow::Result;
use sha2::{Digest as Sha2Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use walkdir::WalkDir;

/// Content-addressed blob storage using file system
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
    max_size: u64,
}

impl BlobStore {
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
                Error::new(ErrorCode::BlobIoError, format!("failed to create directory: {}", e))
            })?;
        }

        // Write atomically using temp file
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, bytes).map_err(|e| {
            Error::new(ErrorCode::BlobIoError, format!("failed to write blob: {}", e))
        })?;

        fs::rename(&temp_path, &path).map_err(|e| {
            Error::new(ErrorCode::BlobIoError, format!("failed to rename blob: {}", e))
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
                Error::new(ErrorCode::BlobIoError, format!("failed to read blob: {}", e))
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
                Error::new(ErrorCode::BlobIoError, format!("failed to delete blob: {}", e))
            }
        })
    }

    /// List all blob digests
    pub fn list_all(&self) -> Vec<Digest> {
        let mut digests = Vec::new();

        for entry in WalkDir::new(&self.root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
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
}
