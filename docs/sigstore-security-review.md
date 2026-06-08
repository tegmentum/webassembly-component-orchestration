# SigStore trust backend — security review scoping & self-audit

Scope: the offline Sigstore verification integration added in
`libs/trust-backends` (`SigStoreTrustBackend`) and its host wiring. This
document is a self-audit + a checklist to drive a deeper, billed
`/code-review ultra` (multi-agent) pass. It is **not** a substitute for that
review — the cryptographic core lives in a third-party crate and the
identity/trust-root policy is security-critical.

## What we own vs. what we delegate

- **Delegated** to `sigstore-verify` 0.8 (reference Rust implementation): bundle
  parsing, Fulcio certificate-chain validation, SCT verification, Rekor
  transparency-log inclusion-proof / checkpoint / SET verification, the
  signature-over-artifact cryptographic check, and CVE-2022-36056-style
  consistency checks. `default-features = false` drops the TUF client and the
  rustls TLS backend (no `rustls` TLS impl is compiled). **Caveat:** it does
  *not* remove the HTTP client — see "Dependency surface" below.
- **Owned** by us (`SigStoreTrustBackend::verify`): mapping the orchestrator's
  `verify(digest, bytes, signature)` contract onto the crate; the digest
  precondition; trust-root selection (embedded production vs. configured file);
  the identity allow-list policy; and result mapping to `VerificationMetadata`.

## Trust model

`signature` is a Sigstore **bundle** (`*.sigstore.json`, v0.1–0.3) carrying the
signing cert, the signature, and Rekor proofs. `bytes` is the artifact. Trust
anchors come from the **embedded Sigstore production trusted root** (shipped by
`sigstore-trust-root`) or a configured `HostConfig.sigstore_trust_root` JSON.
An artifact is trusted iff the bundle verifies against the root **and** (when
configured) the signing identity matches `HostConfig.sigstore_identities`.

## Self-audit findings

### ✅ Artifact ↔ bundle binding holds (the linchpin)

The central question — can an attacker attach a valid-but-unrelated bundle to a
malicious artifact? — is **no** for the message-signature (cosign blob) path:
`verify_impl::verify_hashedrekord_entries(bundle, &artifact)` recomputes
`sha256(bytes)`, requires it to equal the hash in the Rekor entry (which is
authenticated by Rekor's inclusion proof), requires the bundle cert to match the
Rekor entry cert, and cryptographically verifies the signature over the artifact
hash. This runs **unconditionally** (outside all `policy.*` guards), so it is
not disabled by `skip_tlog`. For DSSE the envelope signature is verified
unconditionally in step 7 (`sigstore_crypto::verify_signature` over the PAE),
and for in-toto the artifact hash is matched against the statement subject.
Empirically confirmed by the `tampered` tests (rejected) and the `valid` test
(accepted).

**Hardened (closes residual #1 below for our backend):** the message-signature
crypto check is gated on a `hashedrekord` tlog entry, and bundle validation does
not enforce that the entry kind matches the content type — so a `MessageSignature`
bundle with no `hashedrekord` entry would skip its only signature check. Our
`verify` now rejects exactly that shape before calling the crate (test
`rejects_message_signature_without_hashedrekord_entry`).

### ✅ Cache never stores a failure as success

`TrustStore::verify_with_backend` calls `cache_verification` only **after**
`backend.verify(...)?` returns `Ok` — the `?` short-circuits every failure, so
a rejection is never cached. The cache key is the digest alone (not the
signature/policy), which is sound here because the backend is registered once
with a fixed identity policy; a cached "trusted" for a digest means that
artifact already verified under that policy. (Caveat for future changes: if
identities were reconfigured per-call, the digest-only key could mask the
difference. Default TTL is 24h.)

### ✅ Bundle size is bounded before parsing

`verify` rejects a `signature` payload over `MAX_BUNDLE_BYTES` (1 MiB) before
handing untrusted JSON to the parser (test `rejects_an_oversized_bundle`). Real
bundles are a few KB.

### ✅ Fails closed

`verify` rejects on: digest precondition mismatch, non-UTF-8 / unparseable
bundle, `Err` from the crate, and `Ok(result)` with `success == false`. Default
policy keeps `verify_certificate`, `verify_sct`, and `verify_tlog` all `true`;
we never call the `skip_*` builders.

### ✅ Digest algorithm aligned

`compose-core::compute_digest` is SHA-256, matching the algorithm Sigstore binds
internally. (The precondition is a consistency early-out; security does not
depend on it, since the crate independently binds `sha256(bytes)`.)

### ⚠️ Empty identity allow-list = accept any signer (hardened, not closed)

With no `sigstore_identities`, any artifact bearing a valid signature from the
trusted Sigstore instance is accepted — i.e. any identity Fulcio will certify
(any GitHub account, etc.). For a gate that decides what code runs, the identity
allow-list **is** the access control. We now emit a loud `tracing::warn!` at
construction when the list is empty and document it, but we do not hard-fail
(some deployments gate elsewhere). **Operators must configure
`sigstore_identities` in production.**

### ✅ Integration logic re-read — no bugs found

`SigStoreTrustBackend::verify` and the constructors were re-read line by line:
the policy loop accepts only on `Ok(result)` with `success == true` and rejects
on every other arm (`Ok(!success)`, `Err`, unparseable bundle, digest
mismatch); `integrated_time` (i64) is range-checked into `u64`; the signer
falls back identity → issuer → `"unknown"`. One efficiency note (not a bug):
with N configured identities the full bundle verification is re-run up to N
times, since the crate couples the identity check into `verify`.

### ⚠️ Identity matching is exact-string

`require_identity` / `require_issuer` compare with `==` (no globbing/regex). This
is safe (no fuzzy match), but means SAN identities with run-IDs or refs must be
specified exactly. Confirm the SAN/issuer extraction matches the values
operators will configure (esp. GitHub Actions `…/.github/workflows/…@ref`).

## Dependency surface (lighter local pass: `cargo audit` + `cargo tree`)

Run on the resolved tree (`cargo audit`, `cargo tree`, `cargo deny check
licenses`) on 2026-06-07:

- **Unused HTTP client linked (correcting an earlier claim).** `sigstore-rekor`
  declares `reqwest` as a **mandatory** (non-optional) dependency, so
  `reqwest` + `hyper` + `tokio` are compiled into the host binary via
  `sigstore-verify`, even though our offline path never makes a network call.
  `default-features = false` only deselects the TLS backend (so no `rustls` TLS
  impl is built and reqwest has no working TLS), it cannot drop reqwest itself.
  Impact: larger build + supply-chain/attack surface than necessary; no runtime
  network exposure (we never invoke it). Mitigation: no feature exists to remove
  it — track upstream (request an offline/no-reqwest feature for sigstore-rekor),
  or vendor/patch if the surface is unacceptable.
- **Native crypto (`aws-lc-rs`/`aws-lc-sys`) is linked and used.** Pulled by
  `sigstore-crypto` (signature/hash) and `rustls-webpki` (cert path validation);
  this is the engine doing the real verification. `aws-lc-sys` compiles native
  C, which is a build-toolchain (C compiler/cmake) and supply-chain consideration.
- **`cargo audit`: one advisory — RUSTSEC-2023-0071 (`rsa` Marvin timing
  side-channel, medium, no fix available).** Source is **`pgp` (rPGP)**, not
  SigStore. The Marvin attack targets RSA *private-key* operations; our PGP
  backend only *verifies* with public keys, so practical exposure is negligible.
  No upgrade available regardless. Plus three warnings: `serde_cbor`
  unmaintained, `rand` (RUSTSEC-2026-0097) custom-logger unsoundness, `half` —
  all transitive and low impact.
- **Licenses: all permissive** (Apache-2.0 / MIT / ISC across the sigstore +
  crypto crates). One note: `aws-lc-sys` is `ISC AND (Apache-2.0 OR ISC) AND
  OpenSSL` — the OpenSSL-license clause is worth flagging for license
  compliance. No copyleft (GPL/AGPL) anywhere in the added tree.
  (`cargo deny check advisories` currently fails to load its DB copy on a
  CVSS-4.0 entry — a tooling bug, not our tree; `cargo audit` is authoritative.)

## Resolved in this pass

1. **Message-signature without a hashedrekord tlog entry** — confirmed real
   (binding is kind-gated; `validation.rs` enforces no kind↔content
   correspondence) and **mitigated in our backend** by rejecting such bundles
   before verification. The identity allow-list is the additional backstop.
   Still worth an upstream report so it's fixed at the source.
2. **Bundle as untrusted input / DoS** — `verify` now caps bundle size at 1 MiB
   before parsing. (Parser robustness to *within-bound* pathological JSON is
   still upstream's concern.)
3. **Time handling** — confirmed fail-closed: `determine_validation_time`
   *requires* a verified time source (TSA timestamp or a V1 integrated time with
   an inclusion-promise/SET) and errors otherwise; clock-skew tolerance is 60s.
4. **Cache safety** — confirmed failures are never cached as success (see the
   finding above).

## Still open for the deeper review

1. **Trust-root provenance & rotation.** The embedded production root is pinned
   to the `sigstore-trust-root` crate version. Verify its bytes match the
   official Sigstore TUF root, and define an update cadence (crate bump) for
   root rotations. Treat a custom `sigstore_trust_root` file as security input.
2. **Third-party crypto + version pinning.** `sigstore-verify` (and `x509-cert`,
   `p256`, `aws-lc-rs`, Rekor Merkle code) carry the security weight. `cargo
   audit` now runs as a CI gate (the `cargo-audit` job), with accepted/no-fix
   advisories documented in `.cargo/audit.toml` (currently only the `rsa`
   Marvin advisory, via `pgp`, which our verify-only usage doesn't exercise).
   `Cargo.lock` pins exact versions; we keep caret version reqs (so patch-level
   security fixes flow) rather than `=`-pinning, relying on the audit gate +
   lockfile. Re-review the ignore list on every `pgp`/sigstore upgrade.
3. **Unconditional `reqwest`/`hyper`/`tokio` surface.** Decide whether it's
   acceptable; file an upstream offline-mode request for `sigstore-rekor` or
   vendor/patch (no feature removes it today).
4. **Identity allow-list as the primary control.** The empty-list "accept any
   Fulcio identity" default is hardened with a warning but not closed; production
   deployments must set `sigstore_identities`. Confirm exact-match semantics
   against the real Fulcio SAN/issuer values operators will configure (esp.
   GitHub Actions `…/.github/workflows/…@ref`).

## Checklist to hand the billed `/code-review ultra`

- [x] Re-derive the artifact-binding argument for all bundle kinds (msg-sig,
      DSSE, in-toto) — done above; msg-sig gap mitigated in our backend.
- [ ] Confirm exact-match identity semantics vs. real Fulcio SAN/issuer values.
- [ ] Verify embedded trusted-root bytes against the official Sigstore root;
      decide rotation policy.
- [x] Bound + reject malformed/oversized bundles (size cap + tests).
- [x] `cargo audit` wired as a CI gate (`.cargo/audit.toml` documents the one
      accepted no-fix advisory). `cargo deny`'s DB copy needs a newer version.
- [ ] Decide whether the unconditionally-linked `reqwest`/`hyper`/`tokio`
      surface is acceptable; file an upstream offline-mode request for
      `sigstore-rekor` or vendor/patch.
- [x] Confirm fail-closed under every error path and that caching never caches a
      failure as success.
