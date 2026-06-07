# Plan: remaining stubbed features

Three pre-existing stubs remain, all **outside** the (now-complete) runtime-linking
/ `compose:host` work:

1. **HTTP execution** — `exec.serve-http` / `exec.handle-http` (`hosts/wasmtime/src/exec.rs:590`).
2. **SigStore trust backend** — `libs/compose-core/src/trust/backends.rs:88` (stub).
3. **PGP trust backend** — `libs/compose-core/src/trust/backends.rs:158` (returns `NotImplemented`).

This document plans each. They are independent and can be done in any order; suggested
order is **PGP → HTTP → SigStore** (increasing effort/risk).

## Cross-cutting decision: where the trust backends live

`compose-core` is intentionally **wasm-clean** — it builds for `wasm32-wasip2` and only
pulls `sha2`, `hex`, `getrandom` (wasi backend), and `ed25519-dalek`. SigStore (X.509
cert chains, ECDSA-P256, Merkle inclusion proofs, optional HTTP) and PGP (rPGP/Sequoia)
need heavy, often native, dependencies. Putting them directly in `compose-core` would
break the wasm build.

**Decision (recommended): a new native crate `libs/trust-backends`** that depends on
`compose-core` (for the `TrustBackend` trait + `VerificationMetadata`) and implements the
SigStore and PGP backends. The wasmtime host registers them at startup via
`TrustStore::register_backend`. `compose-core` keeps only the `Dev` backend and the trait,
staying wasm-clean. The current SigStore/PGP **stubs are deleted from `compose-core`**.

- Alternative A: feature-gate the backends in `compose-core` (default off). Rejected —
  it still drags the dep tree into a crate that must stay portable, and `cargo test --all`
  with `--all-features` (our CI) would compile them anyway.
- This requires `TrustStore::with_ttl` to **stop auto-registering** `SigStoreTrustBackend`
  (it currently does at `trust.rs:54`); the host registers backends instead.

The `TrustBackend` trait is already the clean extension point:
```rust
pub trait TrustBackend: Send + Sync {
    fn scheme(&self) -> &str;
    fn verify(&self, digest: &Digest, bytes: &[u8], signature: &[u8])
        -> Result<VerificationMetadata, Error>;
}
```
Verification is dispatched by scheme (`TrustStore::verify_with_backend`), so new backends
need no changes to the dispatch/caching layer.

---

## Item 1 — PGP trust backend (smallest)

**Goal.** `verify(digest, bytes, signature)` checks a detached OpenPGP signature over
`bytes` against a keyring, returning the signer identity.

**Library.** Use **`pgp` (rPGP)** — pure Rust, no native nettle (unlike `sequoia-openpgp`),
so it builds cleanly host-side. Lives in `libs/trust-backends`.

**Steps.**
1. Create `libs/trust-backends` (native crate, depends on `compose-core`, `pgp`, `anyhow`).
2. Move `PgpTrustBackend` there. `new(keyring_path)` loads armored public keys from the
   path (a file or dir of `.asc`).
3. `verify`: parse `signature` as an armored/binary detached signature, verify it over
   `bytes` against each loaded key, on success extract the signing key's fingerprint +
   primary user-id → `VerificationMetadata { signer, timestamp, backend: "pgp" }`. Keep
   the existing `digest == compute_digest(bytes)` precondition.
4. Delete the `compose-core` PGP stub; host registers `PgpTrustBackend` when a keyring path
   is configured (new `HostConfig.pgp_keyring: Option<PathBuf>`).
5. Tests: generate a key + detached sig with `gpg` (or rPGP in a `build`/fixture step),
   commit fixtures under `libs/trust-backends/tests/fixtures/`, assert verify accepts a
   good sig and rejects a tampered artifact / wrong key.

**Risks.** rPGP API churn; keyring format handling. **Effort: ~0.5–1 day.**

---

## Item 2 — HTTP execution (`serve-http` / `handle-http`)

**Goal.** Run a composed component that exports `wasi:http/incoming-handler@0.2.x` and
(a) handle a single request (`handle-http`), (b) serve many over a port (`serve-http`).

**Library.** Add **`wasmtime-wasi-http`** (matching wasmtime 45), gated behind the existing
`http-server` feature alongside `tokio` + `hyper`. `wasi:http` is a standard WASI world;
`wasmtime-wasi-http` provides the host implementation + `ProxyPre` to drive
`incoming-handler`.

**Key wrinkle: async.** `wasi:http` is async; the rest of `exec.rs` is sync. The HTTP path
needs a `tokio` runtime; `handle_http` will `block_on` an async driver. Keep this isolated
behind the feature so the default sync build is unaffected.

**Steps.**
1. Deps (feature `http-server`): `wasmtime-wasi-http = "45"`, `hyper`, `tokio`,
   `http-body-util`. Enable wasmtime `async` where needed.
2. `handle_http(plan, request)`:
   - compose the plan (as `run_cli` does), load the component;
   - build an async `Store` with `WasiCtx` + `WasiHttpCtx`; add `wasmtime_wasi::add_to_linker_async`
     + `wasmtime_wasi_http::add_only_http_to_linker_async`;
   - `ProxyPre::new(...)`; construct an `IncomingRequest` from `HttpRequest`
     (method/path/headers/body); call `proxy.wasi_http_incoming_handler().call_handle(...)`
     with a `ResponseOutparam`; collect the `OutgoingResponse` → `HttpResponse`;
   - run it on a `tokio` current-thread runtime via `block_on`.
3. `serve_http(plan, port)`: a `hyper` server whose service, per request, converts
   hyper `Request` → `HttpRequest` → `handle_http` → `HttpResponse` → hyper `Response`.
   Honor plan policy/limits (epoch/fuel/memory from the sandbox-limits work) per request.
4. Wire `composectl` / `main.rs` exec subcommands (`exec serve-http` / `exec handle-http`).
5. Tests: use the existing `examples/hello-http` component — `handle_http` returns its
   routed response; a `serve-http` integration test binds an ephemeral port and curls it
   (gated on the feature + built example).

**Risks.** async/sync boundary; wasi-http version alignment with wasmtime 45; per-request
store setup cost. Streaming bodies (keep v1 buffered — `HttpRequest`/`HttpResponse` are
already `Vec<u8>` bodies). **Effort: ~2–3 days.**

---

## Item 3 — SigStore trust backend (largest)

**Goal.** Verify a Sigstore-signed artifact and return the certificate identity.

**Decision: offline bundle verification (recommended).** Modern Sigstore **bundles**
(`*.sigstore.json` / protobuf, bundle format ≥ v0.2) are self-contained: they carry the
Fulcio signing certificate, the signature, and the Rekor **inclusion proof** + signed entry
timestamp (SET). Verification needs **no network**:
1. verify the artifact signature with the cert's public key (ECDSA-P256 / Ed25519);
2. verify the leaf cert chains to the **Fulcio root** (pinned/embedded TUF trust root);
3. verify the **Rekor inclusion proof** (Merkle) and SET against Rekor's public key;
4. enforce an **identity policy** (cert SAN identity + OIDC issuer must match config).

Online verification (calling Fulcio/Rekor) is explicitly out of scope — it needs a host
HTTP client and adds availability coupling; bundles already contain the proofs.

**Library options.**
- The `sigstore` crate (rustls/async, fairly heavy) — fastest path but large surface.
- Hand-rolled with `x509-cert`/`der`, `p256`/`ecdsa`, `rs-merkle` + `sigstore-protobuf-specs`
  — more control, smaller, more work.
Recommend starting with the `sigstore` crate's **bundle verification** API if it fits sync
use; otherwise the hand-rolled path. Lives in `libs/trust-backends` (native).

**Steps.**
1. Define the on-the-wire `signature` payload = a Sigstore **bundle** (the `verify` arg).
2. Trust root: embed a pinned Fulcio root + Rekor public key (and/or load a TUF root from
   config: `HostConfig.sigstore_trust_root: Option<PathBuf>`). Document update cadence.
3. Identity policy config: `HostConfig.sigstore_identities: Vec<{ san, issuer }>`.
4. Implement `verify`: parse bundle → steps 1–4 above → `VerificationMetadata { signer =
   cert SAN identity, timestamp = SET time, backend: "sigstore" }`. Keep the digest
   precondition.
5. Delete the `compose-core` SigStore stub; host registers the real backend.
6. Tests: commit real bundle fixtures (signed test artifact + bundle) and assert: valid
   bundle accepts; tampered artifact, wrong identity, and bad inclusion proof all reject.

**Risks (significant).** Bundle/format versioning; trust-root management + rotation;
identity-policy semantics; cert-chain + Merkle-proof correctness (security-critical — get
review). **Effort: ~1–2 weeks.** This is a real security feature; treat the crypto/cert
paths as needing careful review (consider `/security-review`).

---

## Sequencing & shared setup

1. **Bootstrap `libs/trust-backends`** + make `TrustStore::with_ttl` stop auto-registering
   SigStore (host registers backends). Add the `HostConfig` fields as they're needed.
2. **PGP** (small, proves the host-registration pattern).
3. **HTTP** (independent; touches exec + composectl, not trust).
4. **SigStore** (largest; do last, with review).

Each lands behind tests and the pinned-toolchain `fmt`/`clippy -D warnings` gate, like the
runtime-linking work. HTTP is feature-gated (`http-server`); the trust backends are a new
crate so they don't affect `compose-core`'s wasm build.
