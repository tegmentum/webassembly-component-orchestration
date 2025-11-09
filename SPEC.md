# Compositional WebAssembly Specification (`sys:compose@1.0.0`)

**Author / Maintainer:** Zachary Whitley  
**Governance:** Single maintainer  
**Distribution:** Git canonical source with OCI registry mirrors  
**Version:** `@1.0.0`  
**Canonicalization:** `witcanon:1`, `cborcanon:1`

---

## 1. Overview

This specification defines a **compositional framework for WebAssembly components**.  
It provides deterministic planning, composition, and execution of multi-component graphs with reproducible outputs, pluggable trust and secret systems, and verifiable provenance.

---

## 2. Architectural Goals

1. Deterministic and reproducible composition.
2. Language-neutral representation via canonical WIT and CBOR.
3. Pluggable trust, secret, and policy backends.
4. Optional audit, attestation, and metrics.
5. Hybrid deterministic/nondeterministic execution modes.
6. Portable distribution via Git and OCI registries.

---

## 3. Worlds and Interfaces

| Package | Purpose |
|----------|----------|
| `sys:compose/compositor` | Core composition world. |
| `canon:wit` | Canonical WIT normalization and ID computation. |
| `canon:cbor` | Deterministic CBOR profile and validator. |
| `std:secrets` | Token-based secrets with pluggable backends (PKCS#11, Vault, etc.). |
| `std:metrics` | Runtime metrics (optional). |
| `std:audit` | Audit trail and provenance recording (optional). |
| `std:attest` | Remote attestation (TEE/TPM/SigStore) (optional). |

---

## 4. Canonicalization

### 4.1 Canonical WIT
- NFC normalization, deterministic ordering, structural typing.
- `iface-id` / `world-id` / `package-id` = `sha256("witcanon:1" || canonical_cbor_bytes)`.
- Drops comments and source locations.
- Integer-keyed CBOR encoding, definite lengths, sorted order.

### 4.2 Canonical CBOR Profile
- RFC 8949 canonical form with UTF-8 NFC strings.
- Shortest integer encodings, definite-length containers.
- Used for plans, WIT encodings, cache preimages.

---

## 5. Composition and Execution Flow

1. **Plan** – Declarative graph of components, connections, and policies.  
2. **Validate** – Incremental multi-phase validation (A/B/C modes supported).  
3. **Emit** – Deterministic composition → single component artifact.  
4. **Exec** – Execute composed graph (`run-cli`, `serve-http`, `invoke`).  
5. **Reflect** – Introspect exported functions/types at runtime.

---

## 6. Trust and Secrets

### 6.1 Trust
- `trust.verify(digest, bytes, metadata)` interface.
- Hosts may provide PKI, SigStore, or custom verifiers.
- Signatures optional; enforcement is policy-driven.

### 6.2 Secrets
- Token-indirection model: components receive opaque tokens.
- Backends pluggable (`pkcs11://`, `vault://`, `hsm://`, etc.).
- Namespaced URIs with hybrid fallback (scheme → host default → plan map).
- Plan declares required secrets; host resolves and injects.

---

## 7. Policy and Capabilities

- Hybrid model: plan hints + per-exec configuration, host filters both.  
- Each capability marks `required` or `optional`.  
- Host denials respect flags (hard fail vs. soft omit).  
- Determinism mode (strict / audit / relaxed) controlled by host policy.

---

## 8. Caching and Determinism

| Cache | Key formula | Purpose |
|-------|--------------|----------|
| **Emit key** | `H(plan + digests + "emit:v1")` | Reproducible artifact cache. |
| **Exec key** | `H(plan + digests + "exec:v1" + host_abi + caps + policy + tenant + limits + features)` | Safe runtime reuse. |

Canonical CBOR defines preimage serialization.  
Determinism policy is recorded in both exec-key and audit logs.

---

## 9. Multi-Tenancy and Resource Limits

- Plan may specify `tenant_id`, `cpu`, `mem`, `io`.  
- Hosts **must** enforce ceilings regardless of hints.  
- Tenant scope propagates to exec-key, metrics, and audit.

---

## 10. Errors and Events

### 10.1 Hierarchical Error Domains
`Plan.Invalid`, `Emit.MissingBlob`, `Exec.Trap`, `Trust.Failure`, etc.  
Stable codes and optional structured context.

### 10.2 Events Channel
`sys:events.emit(event)` provides structured telemetry (trace/info/warn/error).  
Functional calls still return `result<T, Error>`.

---

## 11. Observability and Audit

### 11.1 Metrics (`std:metrics`)
- Local collection mandatory; external scraping optional.
- Metrics: counter, gauge, histogram with units and labels.

### 11.2 Audit (`std:audit`)
- Hosts must log locally (plan hash, exec key, tenant, outcome).  
- Optional remote submission for provenance records.

### 11.3 Attestation (`std:attest`)
- Optional interface for proofs (TEE/TPM/SigStore).  
- Required in audit / high-trust mode.

---

## 12. Validation Pipeline

- Incremental with reusable artifacts (C).  
- Fallback A (one-shot) and B (stepwise).  
- Phases:  
  1. Schema & canonical CBOR check  
  2. Availability (digests, blobs)  
  3. Shape validation (WIT IDs)  
  4. Graph structure (imports, cycles)  
  5. Policy & capability filtering  
  6. Dry-link verification  
- Each phase emits structured errors via `events`.

---

## 13. Versioning and Compatibility

- Dual scheme:  
  - **SemVer** for WIT interface stability.  
  - **Internal tags** (`witcanon:1`, `cborcanon:1`) for algorithm versioning.  
- Hybrid compatibility: strict pinning by default, adapters allowed by policy.

---

## 14. Conformance

- **Static Vectors:** canonical WIT + plan examples with expected hashes/IDs.  
- **Executable Suite:** runs hosts through all interfaces, checks behavior, errors, metrics.  
- Produces signed conformance report (optional attestation).

---

## 15. Distribution

- **Canonical Source:** Git repo (signed tags).  
- **OCI Mirror:** `application/vnd.wit.bundle.v1+tar`.  
- **Signature:** Cosign/SigStore provenance manifest.  
- Tools (`composectl`, `wac`, `wasm-tools`) resolve via `git://` or `oci://`.

---

## 16. Governance

- Single-maintainer model; immediate iteration allowed.  
- Breaking interface change → SemVer major bump.  
- Canonicalization algorithm change → new internal tag.

---

## 17. Implementation Roadmap (Condensed)

1. **Repo Setup:** CI, license, spec scaffolding.  
2. **Canonicalization:** `canon:cbor`, `canon:wit`, test vectors.  
3. **Core World:** `sys:compose` WIT + host reference (Wasmtime).  
4. **Validation Pipeline:** phased validation + caching.  
5. **Emit + Exec:** deterministic composition, reflection.  
6. **Trust + Secrets:** pluggable modules, PKCS#11 backend.  
7. **Policy + Tenancy:** limits, exec-key integration.  
8. **Observability:** metrics, audit, attestation.  
9. **Tooling:** `composectl` CLI, dev docs, quickstarts.  
10. **Conformance + OCI Distribution.**

---

## 18. Design Principles

1. **Deterministic when required, flexible when allowed.**  
2. **Pluggable trust, secrets, and policy backends.**  
3. **Reproducible builds through canonicalization.**  
4. **Observable and auditable operations.**  
5. **Portable across runtimes and registries.**

---

## 19. Future Work

- Component-level provenance chaining (Merkle DAG).  
- Multi-authority trust federation.  
- Temporal and epoch-based key revocation for secret tokens.  
- Optional WASI Preview 3 adapter world.  
- Richer reflection (type schemas, interface graphs).  

---

**End of SPEC.md**

