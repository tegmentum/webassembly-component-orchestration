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
not disabled by `skip_tlog`. For DSSE/in-toto the artifact hash is matched
against the statement subject. Empirically confirmed by the `tampered` tests
(rejected) and the `valid` test (accepted).

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

## Residual risks for the deeper review

1. **Message-signature without a hashedrekord tlog entry.** The artifact
   signature crypto for `MessageSignature` runs only inside
   `verify_hashedrekord_entries`, which only processes `kind == "hashedrekord"`
   entries. With `verify_tlog = true` (our default) the bundle must carry a
   Rekor-authenticated inclusion proof, and message signatures are hashedrekord
   in practice — but confirm a crafted bundle pairing a `MessageSignature` with
   a non-hashedrekord (yet Rekor-valid) entry cannot skip the signature check.
2. **Trust-root provenance & rotation.** The embedded production root is pinned
   to the `sigstore-trust-root` crate version. Verify its bytes match the
   official Sigstore TUF root, and define an update cadence (crate bump) for
   root rotations. Treat a custom `sigstore_trust_root` file as security input.
3. **Bundle as untrusted input / DoS.** `Bundle::from_json` parses an
   attacker-controlled JSON blob. Confirm size bounds upstream and the parser's
   resilience to pathological inputs.
4. **Third-party crypto.** `sigstore-verify` (and `x509-cert`, `p256`, Rekor
   Merkle code) carry the actual security weight. Pin versions, track
   advisories (`cargo audit`/`cargo deny`), and re-review on upgrade.
5. **Time handling.** Verification uses the tlog integrated time for cert
   validity; confirm clock-skew tolerance (default 60s) and that a missing
   integrated time can't weaken the validity-window check.

## Checklist to hand the billed `/code-review ultra`

- [ ] Re-derive the artifact-binding argument for **all** bundle kinds (msg-sig,
      DSSE, in-toto), including the residual #1 above.
- [ ] Confirm exact-match identity semantics vs. real Fulcio SAN/issuer values.
- [ ] Verify embedded trusted-root bytes against the official Sigstore root;
      decide rotation policy.
- [ ] Fuzz `Bundle::from_json` / verify with malformed & oversized bundles.
- [x] `cargo audit` on the sigstore dep tree (see Dependency surface);
      `cargo deny` advisory DB needs a newer copy. Still TODO: pin versions.
- [ ] Decide whether the unconditionally-linked `reqwest`/`hyper`/`tokio`
      surface is acceptable; file an upstream offline-mode request for
      `sigstore-rekor` or vendor/patch.
- [ ] Confirm fail-closed under every error path (no `Ok` leak on partial
      verification) and that caching (`TrustStore`) never caches a failure as
      success.
