//! Trust-gated cache for precompiled component artifacts.
//!
//! A [`CompileCache`] sits on top of any [`BlobBackend`] and stores
//! engine-specific machine code (e.g. wasmtime `.cwasm`) keyed by the triple
//! `(component_digest, engine_version, target)`. Because deserializing
//! precompiled wasm produces runnable native code, every cached artifact is
//! authenticated with an HMAC over a host-local secret: a tampered or
//! foreign-engine blob fails verification and is treated as a cache miss
//! rather than being deserialized.
//!
//! This is the wasmtime-free, sqlite-free generalization of sqlink's
//! `host/src/component_blob_cache.rs` — the engine-version key and the HMAC
//! trust model are ported from there. The `engine_version` and `target` are
//! taken as host `&str` so compose-core never needs to link wasmtime to know
//! its version.

use crate::blobs::{compute_digest, BlobBackend};
use crate::types::{Digest, Error, ErrorCode};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Length of the HMAC tag (SHA-256) prepended to every stored artifact.
const TAG_LEN: usize = 32;

/// A trust-gated, content-addressed cache for precompiled component code.
///
/// Generic over the storage [`BlobBackend`]: the framework + ducklink use
/// [`FsBlobStore`](crate::blobs::FsBlobStore); sqlink uses its
/// SQLite-backed `SqliteCasStore`. Both keep compose-core wasmtime-free and
/// sqlite-free.
pub struct CompileCache<B: BlobBackend> {
    backend: B,
    /// Host-local secret authenticating every cached artifact. Never persisted
    /// inside the cache; only the keyed MAC tag is.
    hmac_key: Vec<u8>,
}

impl<B: BlobBackend> CompileCache<B> {
    /// Wrap a backend with the given host-local HMAC secret.
    ///
    /// The secret authenticates artifacts on store and is required to be
    /// byte-identical on verify; a different (or zeroed) secret turns every
    /// lookup into a miss. Hosts typically load this from a 0600 key file
    /// generated once per machine.
    pub fn new(backend: B, hmac_key: impl Into<Vec<u8>>) -> Self {
        Self {
            backend,
            hmac_key: hmac_key.into(),
        }
    }

    /// Borrow the underlying blob backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Compute the cache key for a precompiled artifact:
    /// `sha256(component_digest || engine_version || target)`.
    ///
    /// `engine_version` should encode everything that invalidates the cache on
    /// upgrade (host + wasmtime version, relevant compiler config); `target`
    /// is the machine triple. Both are host strings so compose-core never
    /// links a runtime.
    pub fn cache_key(component_digest: &Digest, engine_version: &str, target: &str) -> Digest {
        let mut material =
            Vec::with_capacity(component_digest.len() + engine_version.len() + target.len() + 2);
        material.extend_from_slice(component_digest);
        // Domain separators keep `(a, b)` from colliding with `(ab, "")`.
        material.push(0x00);
        material.extend_from_slice(engine_version.as_bytes());
        material.push(0x00);
        material.extend_from_slice(target.as_bytes());
        compute_digest(&material)
    }

    /// Wrap `artifact` as a self-authenticating, framed blob
    /// (`hmac_tag(32) || artifact`) bound to the `(component_digest,
    /// engine_version, target)` triple, without touching the backend.
    ///
    /// For artifacts a host stores at a path it already manages (e.g. a
    /// `.cwasm` file next to the source `.wasm`): `seal` on write, [`open`] on
    /// read. The HMAC binds the bytes to both the host secret and the engine
    /// triple, so a tampered or foreign-engine file fails to open.
    ///
    /// [`open`]: Self::open
    pub fn seal(
        &self,
        component_digest: &Digest,
        engine_version: &str,
        target: &str,
        artifact: &[u8],
    ) -> Vec<u8> {
        let key = Self::cache_key(component_digest, engine_version, target);
        let tag = self.tag(&key, artifact);
        let mut framed = Vec::with_capacity(TAG_LEN + artifact.len());
        framed.extend_from_slice(&tag);
        framed.extend_from_slice(artifact);
        framed
    }

    /// Verify and unwrap a blob produced by [`seal`](Self::seal). Returns
    /// `None` (never the bytes) when the frame is malformed or the HMAC does
    /// not authenticate for these inputs — so a caller can safely fall back to
    /// recompiling rather than deserializing untrusted machine code.
    pub fn open(
        &self,
        component_digest: &Digest,
        engine_version: &str,
        target: &str,
        framed: &[u8],
    ) -> Option<Vec<u8>> {
        if framed.len() < TAG_LEN {
            return None;
        }
        let key = Self::cache_key(component_digest, engine_version, target);
        let (tag, artifact) = framed.split_at(TAG_LEN);
        if self.verify(&key, artifact, tag) {
            Some(artifact.to_vec())
        } else {
            None
        }
    }

    /// Authenticate `artifact` under `key` and store it, returning the cache
    /// key. The stored blob is `hmac_tag(32 bytes) || artifact`.
    pub fn store(
        &self,
        component_digest: &Digest,
        engine_version: &str,
        target: &str,
        artifact: &[u8],
    ) -> Result<Digest, Error> {
        let key = Self::cache_key(component_digest, engine_version, target);
        let tag = self.tag(&key, artifact);

        let mut framed = Vec::with_capacity(TAG_LEN + artifact.len());
        framed.extend_from_slice(&tag);
        framed.extend_from_slice(artifact);

        // The blob backend addresses the framed bytes by their own SHA-256.
        // Because a pure CAS cannot be queried by an arbitrary key, we also
        // write a small fixed-shape *alias* blob whose content is
        // `cache_key || framed_digest`; `load` recovers the artifact by finding
        // the alias whose leading bytes equal the recomputed `cache_key`.
        let framed_digest = self.backend.put(&framed)?;
        let mut alias = Vec::with_capacity(key.len() + framed_digest.len());
        alias.extend_from_slice(&key);
        alias.extend_from_slice(&framed_digest);
        self.backend.put(&alias)?;
        Ok(key)
    }

    /// Look up and authenticate a previously stored artifact. Returns
    /// `Ok(None)` on a cache miss, and treats a failed HMAC check as a miss
    /// (never returns unauthenticated bytes).
    pub fn load(
        &self,
        component_digest: &Digest,
        engine_version: &str,
        target: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        let key = Self::cache_key(component_digest, engine_version, target);
        let Some(framed_digest) = self.resolve_alias(&key)? else {
            return Ok(None);
        };
        let framed = match self.backend.get(&framed_digest) {
            Ok(bytes) => bytes,
            // A missing or corrupt underlying blob is a miss, not a hard error.
            Err(e) if e.code == ErrorCode::BlobNotFound => return Ok(None),
            Err(e) if e.code == ErrorCode::BlobDigestMismatch => return Ok(None),
            Err(e) => return Err(e),
        };

        if framed.len() < TAG_LEN {
            return Ok(None);
        }
        let (tag, artifact) = framed.split_at(TAG_LEN);

        // Constant-time verify against the key derived from the trusted inputs.
        if !self.verify(&key, artifact, tag) {
            tracing::warn!(
                "compile-cache HMAC mismatch for key {} — treating as miss",
                hex::encode(&key)
            );
            return Ok(None);
        }
        Ok(Some(artifact.to_vec()))
    }

    /// Whether an authenticated artifact exists for these inputs.
    pub fn contains(
        &self,
        component_digest: &Digest,
        engine_version: &str,
        target: &str,
    ) -> Result<bool, Error> {
        Ok(self
            .load(component_digest, engine_version, target)?
            .is_some())
    }

    // --- alias resolution: compile-key -> framed-blob digest ------------
    //
    // A pure CAS addresses blobs only by `sha256(content)`, so it cannot be
    // queried by an arbitrary compile key. We bridge this by writing a small
    // alias blob of shape `cache_key(32) || framed_digest(32)`. To find the
    // artifact for a key we scan the backend for the alias whose leading 32
    // bytes match. Backends with a richer index (sqlink's `__cas_uri`) may
    // override this path; the scan is the trait-only fallback. The HMAC check
    // in `load` is the real trust gate, so a forged alias still cannot yield
    // unauthenticated bytes.

    fn resolve_alias(&self, key: &Digest) -> Result<Option<Digest>, Error> {
        let key_len = key.len();
        for d in self.backend.list_all() {
            let Ok(bytes) = self.backend.get(&d) else {
                continue;
            };
            if bytes.len() == key_len + TAG_LEN && bytes[..key_len] == key[..] {
                // The trailing bytes are the framed-blob digest.
                return Ok(Some(bytes[key_len..].to_vec()));
            }
        }
        Ok(None)
    }

    // --- HMAC ------------------------------------------------------------

    fn mac(&self, key: &Digest, artifact: &[u8]) -> HmacSha256 {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.hmac_key)
            .expect("HMAC accepts keys of any length");
        // Bind the tag to both the compile key and the artifact bytes so a
        // blob cannot be replayed under a different key.
        mac.update(key);
        mac.update(&[0x00]);
        mac.update(artifact);
        mac
    }

    fn tag(&self, key: &Digest, artifact: &[u8]) -> Vec<u8> {
        self.mac(key, artifact).finalize().into_bytes().to_vec()
    }

    fn verify(&self, key: &Digest, artifact: &[u8], tag: &[u8]) -> bool {
        self.mac(key, artifact).verify_slice(tag).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::FsBlobStore;
    use tempfile::tempdir;

    fn store() -> CompileCache<FsBlobStore> {
        let dir = tempdir().unwrap();
        let backend = FsBlobStore::new(dir.path().to_path_buf(), 16 * 1024 * 1024).unwrap();
        CompileCache::new(backend, b"test-secret-key".to_vec())
    }

    #[test]
    fn test_store_and_load_roundtrip() {
        let cache = store();
        let component = compute_digest(b"my-component-bytes");
        let artifact = b"precompiled machine code here";

        assert!(cache
            .load(&component, "engine-1", "aarch64-macos")
            .unwrap()
            .is_none());

        cache
            .store(&component, "engine-1", "aarch64-macos", artifact)
            .unwrap();

        let loaded = cache.load(&component, "engine-1", "aarch64-macos").unwrap();
        assert_eq!(loaded.as_deref(), Some(&artifact[..]));
        assert!(cache
            .contains(&component, "engine-1", "aarch64-macos")
            .unwrap());
    }

    #[test]
    fn test_engine_version_invalidates() {
        let cache = store();
        let component = compute_digest(b"comp");
        cache
            .store(&component, "engine-1", "aarch64-macos", b"art")
            .unwrap();

        // Different engine version -> different key -> miss.
        assert!(cache
            .load(&component, "engine-2", "aarch64-macos")
            .unwrap()
            .is_none());
        // Different target -> miss.
        assert!(cache
            .load(&component, "engine-1", "x86_64-linux")
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_wrong_key_is_a_miss_not_a_leak() {
        let dir = tempdir().unwrap();
        let backend = FsBlobStore::new(dir.path().to_path_buf(), 16 * 1024 * 1024).unwrap();
        let component = compute_digest(b"comp");

        // Store with one secret.
        let writer = CompileCache::new(backend.clone(), b"secret-A".to_vec());
        writer
            .store(&component, "engine-1", "t", b"trusted-code")
            .unwrap();

        // Read with a different secret: HMAC must fail -> None (no bytes leak).
        let reader = CompileCache::new(backend, b"secret-B".to_vec());
        assert!(reader.load(&component, "engine-1", "t").unwrap().is_none());
    }

    #[test]
    fn test_seal_open_roundtrip_and_tamper() {
        let cache = store();
        let component = compute_digest(b"comp");
        let art = b"precompiled bytes";

        let sealed = cache.seal(&component, "engine-1", "t", art);
        assert_eq!(
            cache.open(&component, "engine-1", "t", &sealed).as_deref(),
            Some(&art[..])
        );

        // Wrong engine triple -> no open.
        assert!(cache.open(&component, "engine-2", "t", &sealed).is_none());

        // Tampered payload -> no open.
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 0xFF;
        assert!(cache.open(&component, "engine-1", "t", &bad).is_none());

        // Too short -> no open.
        assert!(cache.open(&component, "engine-1", "t", b"short").is_none());
    }

    #[test]
    fn test_cache_key_domain_separation() {
        let c = compute_digest(b"c");
        // ("ab","") must not collide with ("a","b").
        let k1 = CompileCache::<FsBlobStore>::cache_key(&c, "ab", "");
        let k2 = CompileCache::<FsBlobStore>::cache_key(&c, "a", "b");
        assert_ne!(k1, k2);
    }
}
