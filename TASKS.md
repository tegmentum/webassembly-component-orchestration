# 🧠 CODEx_TASKS.md
Machine-readable roadmap for building the **Compositional WebAssembly System**.

Each milestone is self-contained and can be executed by an AI-assisted developer or autonomous agent.  
Dependencies and acceptance criteria are explicitly defined.

---

## M0  — Repository Scaffolding
**Goal:** Create the initial project structure and metadata.

**Tasks**
- [ ] Initialize Git repository and basic CI (GitHub Actions / Cargo build).
- [ ] Add LICENSE (Apache 2.0) and CODEOWNERS.
- [ ] Create directories:  
  `/spec`, `/wit`, `/hosts`, `/tools`, `/examples`, `/conformance`.
- [ ] Copy in `SPEC.md`, `README.md`, `CODEx_TASKS.md`.
- [ ] Add `.editorconfig` and `.gitignore`.

**Acceptance Criteria**
- `git status` is clean.
- CI builds without errors.
- All top-level docs render correctly on GitHub.

---

## M1  — Canonicalization Layer
**Goal:** Deterministic WIT and CBOR encoders.

**Deliverables**
1. `wit/canon-cbor/` – deterministic CBOR encoder/decoder (lib + tests).  
2. `wit/canon-wit/` – WIT parser → canonical CBOR → SHA-256 IDs.  
3. Test vectors under `conformance/vectors/canon/`.
4. Baseline plan vector (`conformance/vectors/hello-plan.cbor`) with digest.

**Tasks**
- [ ] Implement RFC 8949 canonical order.
- [ ] Implement `witcanon:1` hashing (`sha256("witcanon:1" || bytes)`).
- [ ] Provide CLI tool `canonctl` with:  
  `wit to-cbor`, `wit id`, `plan validate-cbor`.

**Acceptance Criteria**
- Canonical round-trip produces identical hashes across runs/languages.
- Test vectors match across Rust/JS implementations.

---

## M2  — Core World (`sys:compose`)
**Goal:** Define and wire the main WIT interfaces.

**Deliverables**
- `wit/sys-compose/` containing interfaces:  
  `plan`, `emit`, `exec`, `blobs`, `trust`, `events`.  
- Canonical CBOR schema for `plan v1` in `spec/plan.cddl`.
- Hierarchical error codes registry `spec/errors.md`.

**Tasks**
- [ ] Implement `plan.serialize/deserialize`.
- [ ] Integrate `canon:wit` for shape verification.
- [ ] Define event record format (trace/info/warn/error).

**Acceptance Criteria**
- CBOR plans validate and round-trip.
- Errors emit structured codes (e.g. `Plan.InvalidSchema`).

---

## M3  — Reference Host (Wasmtime)
**Goal:** Minimal working host with compose + exec.

**Deliverables**
- `hosts/wasmtime/` crate implementing `sys:compose`.
- File-backed CAS for `blobs`.
- Incremental validation pipeline.

**Tasks**
- [ ] Implement `emit.compose` using `wasm-tools`.
- [ ] Implement `exec.run-cli`, `exec.invoke`, `exec.serve-http`.
- [ ] Implement reflection APIs in `exec` (`list-exports`, `describe-export`).
- [ ] Wire `events` emission and logging.

**Acceptance Criteria**
- Example plan builds → runnable Wasm component.
- Reflection shows export names and types.

---

## M4  — Trust & Secrets Pluggability
**Goal:** Secure supply-chain integration.

**Deliverables**
- `wit/std-secrets/` (token model C with pluggable backends).  
- `wit/trust/` interface for artifact verification.  
- Implement PKCS#11 and dev secret backends.  
- Implement SigStore trust plugin.

**Tasks**
- [ ] Host: resolve plan secret IDs → opaque tokens.  
- [ ] Validate trust signatures during emit and exec.  
- [ ] Cache verified digests by plan hash.

**Acceptance Criteria**
- Plan requiring secrets runs with PKCS#11 backend.  
- Signed artifacts verify successfully.

---

## M5  — Policy & Tenancy
**Goal:** Controlled execution environments.

**Deliverables**
- Capability model (`required`/`optional`).  
- Tenant scope & limits structs.  
- Integration into `exec-key` computation.

**Tasks**
- [ ] Host: filter capabilities per policy.  
- [ ] Enforce resource quotas (cpu/mem/io).  
- [ ] Include `tenant_id` in audit records and cache keys.

**Acceptance Criteria**
- Denied optional caps soft-degrade; denied required caps fail.  
- Tenant-scoped cache isolation verified by tests.

---

## M6  — Observability
**Goal:** Unified metrics, audit, and attestation.

**Deliverables**
- `wit/std-metrics/`, `wit/std-audit/`, `wit/std-attest/`.  
- Local logging + optional remote export.  

**Tasks**
- [ ] Record runtime metrics for compose & exec.  
- [ ] Implement `audit.record` and optional `attest.attest/verify`.  
- [ ] Include determinism mode and trust policy in records.

**Acceptance Criteria**
- Metrics visible via `composectl metrics list`.  
- Audit entries contain plan hash + exec-key.  
- Attestation proof verifies with host public key.

---

## M7  — CLI Tooling (`composectl`)
**Goal:** User-facing control plane.

**Deliverables**
- `tools/composectl/` Rust binary.  
- Subcommands:  
  `plan`, `emit`, `exec`, `secrets`, `trust`, `reflect`, `metrics`, `conformance`.

**Tasks**
- [ ] Wire to host via WIT bindings.  
- [ ] Add JSON and TOML output formats.  
- [ ] Support both Git and OCI package sources.

**Acceptance Criteria**
- `composectl emit build` and `exec run` work on examples.  
- `composectl reflect` prints exports and types.

---

## M8  — Conformance Suite
**Goal:** Verify host compliance.

**Deliverables**
- `conformance/runner/` with host adapter API.  
- Static vectors (`conformance/vectors/`).  
- Signed report artifact.

**Tasks**
- [ ] Implement phased tests (plan→emit→exec).  
- [ ] Validate error codes and event sequence.  
- [ ] Generate summary JSON + optional attestation.

**Acceptance Criteria**
- Reference host passes all tests.  
- Report includes metrics and audit digest.

---

## M9  — Distribution & Release
**Goal:** Public release and registry integration.

**Deliverables**
- Git signed tag `v1.0.0`.  
- OCI bundle (`application/vnd.wit.bundle.v1+tar`).  
- Cosign/SigStore signature and provenance manifest.

**Tasks**
- [ ] Package WIT bundles from `wit/` directory.  
- [ ] Push to `oci://registry.example.com/wit/sys/compose:v1.0.0`.  
- [ ] Verify OCI digest matches Git commit.

**Acceptance Criteria**
- OCI pull and Git clone yield identical content hashes.

---

## M10  — Examples & Demos
**Goal:** Showcase usage patterns.

**Deliverables**
- `examples/hello-cli` and `examples/hello-http`.  
- Secrets demo (PKCS#11 + Vault).  
- Trust demo (unsigned vs signed).  
- Determinism demo (strict vs relaxed).

**Acceptance Criteria**
- Each example builds and runs under reference host.  
- Logs, metrics, and audit entries are generated as expected.

---

## M11  — Hardening & QA
**Goal:** Security and performance stability.

**Tasks**
- [ ] Fuzz plan parser and WIT canonicalizer.  
- [ ] Add graph/byte size limits and DOS guards.  
- [ ] Security audit of capability and secret flows.  
- [ ] Performance profiling and cache optimization.

**Acceptance Criteria**
- No panic or OOM on invalid input.  
- Compose time and memory usage benchmarks within targets.

---

## Global Acceptance Milestone
System considered **v1.0 compliant** when:
- Reference host passes conformance suite with signed report.  
- OCI bundle published and verifiable.  
- `composectl` can plan → emit → exec a multi-component example with trust and secrets enabled.

---

**End of CODEx_TASKS.md**

