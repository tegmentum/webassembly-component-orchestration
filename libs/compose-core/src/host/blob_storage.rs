//! Blob storage capability — content-addressed storage for components and plans.
use crate::types::{Digest, Error};
use std::sync::Arc;

/// Content-addressed blob storage.
///
/// Each blob is keyed by its SHA-256 digest. `put` returns the digest the
/// implementation computed for the bytes, which the caller must use to fetch
/// the blob later via `get`.
///
/// In the eventual wasm-orchestrator design, this trait will be satisfied by
/// an imported `compose:blob-store` (or `wasi:keyvalue`-shaped) interface.
pub trait BlobStorage: Send + Sync {
    /// Store bytes and return the digest the implementation computed.
    fn put(&self, bytes: &[u8]) -> Result<Digest, Error>;

    /// Fetch a blob by digest. Implementations should verify integrity.
    fn get(&self, digest: &Digest) -> Result<Vec<u8>, Error>;

    /// Whether a blob with this digest is present.
    fn has(&self, digest: &Digest) -> bool;

    /// Size of a stored blob without retrieving content.
    fn size(&self, digest: &Digest) -> Option<u64>;

    /// Remove a blob by digest.
    fn delete(&self, digest: &Digest) -> Result<(), Error>;

    /// List every digest the store knows about.
    fn list_all(&self) -> Vec<Digest>;
}

/// Shared, thread-safe handle to a blob store.
pub type SharedBlobs = Arc<dyn BlobStorage>;
