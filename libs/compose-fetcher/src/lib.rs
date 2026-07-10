//! Blocking multi-scheme fetcher for composition-plan artifacts.
//!
//! Callers hand a `source` URL and (optionally) an expected SHA-256 digest.
//! We resolve the URL through a matching scheme handler, verify the digest
//! if given, and cache the bytes on disk keyed by digest so repeated fetches
//! of the same content are a single file read.
//!
//! Design constraints:
//! * Synchronous. The wf plugin drives composition from inside a wasmtime
//!   host callback, and pulling a tokio runtime into that stack is an
//!   avoidable liability. `ureq` handles HTTP without one; the optional
//!   OCI path spins up a scoped runtime only for the fetch call.
//! * Digest-first identity. If the plan carries a digest, the fetcher
//!   refuses to return content that does not match — supply-chain integrity
//!   is the whole reason for putting digests in plans in the first place.
//! * Content-addressed cache. Cache keys are the digest, not the source
//!   URL, so a component reachable at multiple URLs is stored once.

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tracing::{debug, warn};

/// Canonical 32-byte SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn from_bytes(b: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(b);
        Sha256Digest(h.finalize().into())
    }
    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }
    /// Accept the two forms plans use in practice: bare hex, or `sha256:` prefix.
    pub fn parse(s: &str) -> Result<Self> {
        let hex = s.strip_prefix("sha256:").unwrap_or(s);
        let bytes = hex::decode(hex).with_context(|| format!("digest not valid hex: {s}"))?;
        if bytes.len() != 32 {
            bail!("digest must be 32 bytes / 64 hex chars, got {}", bytes.len());
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(Sha256Digest(out))
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Fetcher configuration. Defaults resolve `$XDG_CACHE_HOME/compose-fetcher/`
/// as the on-disk cache, a permissive-but-finite HTTP timeout, and a public
/// IPFS gateway; production embedders should override at least the gateway.
#[derive(Debug, Clone)]
pub struct FetcherConfig {
    pub cache_dir: PathBuf,
    pub http_timeout: Duration,
    pub max_response_bytes: u64,
    /// URL prefix used to resolve `ipfs://<cid>[/path]` when no local gateway
    /// is configured. Defaults to https://ipfs.io/ipfs — swap for a private
    /// or trusted gateway in production. `oci://` is handled by `oci-client`
    /// directly and does not use this field.
    pub ipfs_gateway: String,
}

impl Default for FetcherConfig {
    fn default() -> Self {
        let cache_dir = dirs::cache_dir()
            .map(|d| d.join("compose-fetcher"))
            .unwrap_or_else(|| PathBuf::from("/tmp/compose-fetcher"));
        Self {
            cache_dir,
            http_timeout: Duration::from_secs(60),
            max_response_bytes: 256 * 1024 * 1024, // 256 MB safety ceiling
            ipfs_gateway: "https://ipfs.io/ipfs".into(),
        }
    }
}

/// The main entry point. Given a `source` URL and optionally the expected
/// digest recorded in the plan, return the artifact bytes. Fetches at most
/// once per unique digest (cache hit) and refuses on integrity mismatch.
pub fn fetch(
    source: &str,
    expected: Option<Sha256Digest>,
    cfg: &FetcherConfig,
) -> Result<Vec<u8>> {
    // Cache lookup happens BEFORE the network call when we already have the
    // digest — no reason to hit the wire for content we've stored before.
    if let Some(d) = expected {
        if let Some(bytes) = read_from_cache(&cfg.cache_dir, &d)? {
            debug!(source, digest = %d.as_hex(), "compose-fetcher cache hit");
            return Ok(bytes);
        }
    }

    let bytes = fetch_raw(source, cfg)?;

    // Digest verification. Refuse mismatch — a plan that pins a digest is
    // asking the fetcher to enforce it, and silently returning other bytes
    // would defeat the point.
    let actual = Sha256Digest::from_bytes(&bytes);
    if let Some(want) = expected {
        if actual != want {
            bail!(
                "compose-fetcher: digest mismatch for {source}: got {}, expected {}",
                actual.as_hex(),
                want.as_hex()
            );
        }
    }

    // Populate the cache under the actual digest whether or not the plan
    // specified one — future fetches with the same digest hit disk.
    if let Err(e) = write_to_cache(&cfg.cache_dir, &actual, &bytes) {
        warn!(?e, "compose-fetcher: cache write failed; continuing without cache");
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Scheme dispatch
// ---------------------------------------------------------------------------

fn fetch_raw(source: &str, cfg: &FetcherConfig) -> Result<Vec<u8>> {
    if let Some(path) = source.strip_prefix("file://") {
        return fetch_file(path);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        return fetch_http(source, cfg);
    }
    if let Some(rest) = source.strip_prefix("ipfs://") {
        return fetch_ipfs(rest, cfg);
    }
    if source.starts_with("oci://") {
        #[cfg(feature = "oci")]
        {
            return fetch_oci(source, cfg);
        }
        #[cfg(not(feature = "oci"))]
        {
            bail!("compose-fetcher: oci:// requires the `oci` feature");
        }
    }
    bail!("compose-fetcher: unsupported source scheme in {source}");
}

fn fetch_file(path: &str) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("reading local file {path}"))
}

fn fetch_http(url: &str, cfg: &FetcherConfig) -> Result<Vec<u8>> {
    let agent = ureq::AgentBuilder::new()
        .timeout(cfg.http_timeout)
        .build();
    let resp = agent.get(url).call().with_context(|| format!("HTTP GET {url}"))?;
    if resp.status() < 200 || resp.status() >= 300 {
        bail!("compose-fetcher: HTTP {} for {}", resp.status(), url);
    }
    let mut buf = Vec::new();
    resp.into_reader()
        .take(cfg.max_response_bytes)
        .read_to_end(&mut buf)
        .with_context(|| format!("reading body from {url}"))?;
    if buf.len() as u64 == cfg.max_response_bytes {
        // Hit the ceiling exactly — the response might be truncated. Refuse
        // rather than silently return partial content.
        bail!(
            "compose-fetcher: response from {url} hit the {}-byte size ceiling",
            cfg.max_response_bytes
        );
    }
    Ok(buf)
}

fn fetch_ipfs(rest: &str, cfg: &FetcherConfig) -> Result<Vec<u8>> {
    // `ipfs://cid[/path]` -> `<gateway>/<cid>[/path]`. We don't try to be
    // a native IPFS client — a trusted gateway is the pragmatic default.
    let url = format!("{}/{rest}", cfg.ipfs_gateway.trim_end_matches('/'));
    fetch_http(&url, cfg)
}

// ---------------------------------------------------------------------------
// OCI (optional — gated on the `oci` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "oci")]
fn fetch_oci(source: &str, _cfg: &FetcherConfig) -> Result<Vec<u8>> {
    use oci_client::{Client, Reference, secrets::RegistryAuth, manifest::WASM_LAYER_MEDIA_TYPE};

    // `oci://registry/image:tag` -> `registry/image:tag`
    let raw = source
        .strip_prefix("oci://")
        .ok_or_else(|| anyhow!("expected oci:// prefix"))?;
    let reference: Reference = raw
        .parse()
        .with_context(|| format!("parsing OCI reference {raw}"))?;

    // Scope a tiny tokio current-thread runtime just for this call so the
    // rest of the fetcher stays sync. This keeps async out of the caller's
    // stack while still allowing oci-client's async API.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("scoped oci runtime")?;

    rt.block_on(async move {
        let client = Client::default();
        // We want the whole component wasm as one blob. `pull` fetches the
        // image and gives us its layers. For a wasm-in-OCI image (per the
        // Bytecode Alliance packaging convention) there is a single layer
        // whose media type is the WASM layer type; take that one.
        let image = client
            .pull(&reference, &RegistryAuth::Anonymous, vec![WASM_LAYER_MEDIA_TYPE])
            .await
            .with_context(|| format!("OCI pull {source}"))?;
        let mut layers = image.layers;
        let layer = layers
            .pop()
            .ok_or_else(|| anyhow!("OCI image {source} has no layers"))?;
        Ok::<_, anyhow::Error>(layer.data)
    })
}

// ---------------------------------------------------------------------------
// Content-addressed cache
// ---------------------------------------------------------------------------

fn cache_path(cache_dir: &Path, d: &Sha256Digest) -> PathBuf {
    // Fan out into 2-char sharded subdirs so directories don't accumulate
    // tens of thousands of siblings when the cache grows.
    let hex = d.as_hex();
    let (a, b) = hex.split_at(2);
    let (c, _) = b.split_at(2);
    cache_dir.join(a).join(c).join(&hex)
}

fn read_from_cache(cache_dir: &Path, d: &Sha256Digest) -> Result<Option<Vec<u8>>> {
    let p = cache_path(cache_dir, d);
    match fs::read(&p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading cache entry {}", p.display())),
    }
}

fn write_to_cache(cache_dir: &Path, d: &Sha256Digest, bytes: &[u8]) -> Result<()> {
    let p = cache_path(cache_dir, d);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    // Write to a sibling temp file and rename so a partial write can never
    // leave a corrupt cache entry visible.
    let tmp = p.with_extension("tmp");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &p).with_context(|| format!("renaming {} -> {}", tmp.display(), p.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cfg_in(tmp: &Path) -> FetcherConfig {
        FetcherConfig {
            cache_dir: tmp.join("cache"),
            ..Default::default()
        }
    }

    #[test]
    fn digest_round_trip() {
        let d = Sha256Digest::from_bytes(b"hello");
        let parsed = Sha256Digest::parse(&d.as_hex()).unwrap();
        assert_eq!(d, parsed);
        let prefixed = format!("sha256:{}", d.as_hex());
        assert_eq!(Sha256Digest::parse(&prefixed).unwrap(), d);
    }

    #[test]
    fn digest_length_enforced() {
        assert!(Sha256Digest::parse("cafebabe").is_err());
    }

    #[test]
    fn fetches_file_scheme() {
        let tmp = TempDir::new().unwrap();
        let payload = b"file-scheme payload";
        let src_path = tmp.path().join("component.wasm");
        fs::write(&src_path, payload).unwrap();
        let src_url = format!("file://{}", src_path.display());
        let got = fetch(&src_url, None, &cfg_in(tmp.path())).unwrap();
        assert_eq!(&got, payload);
    }

    #[test]
    fn digest_mismatch_refused() {
        let tmp = TempDir::new().unwrap();
        let payload = b"expected content";
        let src_path = tmp.path().join("c.wasm");
        fs::write(&src_path, payload).unwrap();
        let src_url = format!("file://{}", src_path.display());
        let wrong = Sha256Digest::from_bytes(b"different content");
        let err = fetch(&src_url, Some(wrong), &cfg_in(tmp.path())).unwrap_err();
        assert!(
            format!("{err}").contains("digest mismatch"),
            "expected digest mismatch error, got: {err}"
        );
    }

    #[test]
    fn cache_serves_second_fetch_after_source_gone() {
        let tmp = TempDir::new().unwrap();
        let payload = b"caching this one";
        let src_path = tmp.path().join("c.wasm");
        fs::write(&src_path, payload).unwrap();
        let src_url = format!("file://{}", src_path.display());
        let digest = Sha256Digest::from_bytes(payload);
        let cfg = cfg_in(tmp.path());

        // First fetch populates the cache.
        let a = fetch(&src_url, Some(digest), &cfg).unwrap();
        assert_eq!(&a, payload);

        // Remove the source; the cache should still serve the digest.
        fs::remove_file(&src_path).unwrap();
        let b = fetch("file:///dev/nonexistent", Some(digest), &cfg).unwrap();
        assert_eq!(&b, payload);
    }

    #[test]
    fn unsupported_scheme_errors_cleanly() {
        let tmp = TempDir::new().unwrap();
        let err = fetch("gopher://old.example.com/wasm", None, &cfg_in(tmp.path())).unwrap_err();
        assert!(format!("{err}").contains("unsupported source scheme"));
    }
}
