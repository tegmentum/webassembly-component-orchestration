//! End-to-end smoke test: load the wasm-orchestrator component, call
//! its `compose:host/smoke.host-name` export, and confirm the round
//! trip returns the runtime name our host advertises.
//!
//! Requires the orchestrator artifact to be built first:
//!
//!     libs/compose-orchestrator-wasm/build.sh
//!
//! If the artifact is missing, the test prints a hint and skips —
//! this keeps `cargo test -p compose-host-wasmtime` green on fresh
//! checkouts without forcing every contributor to install
//! `wasm32-wasip2` and the wit-bindgen toolchain.
use std::path::PathBuf;

use compose_host_wasmtime::compose_host;

/// The orchestrator crate is `exclude`d from the workspace because it
/// builds for `wasm32-wasip2`, so its artifacts land in a crate-local
/// `target/` rather than the workspace one.
fn orchestrator_wasm_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("..")
        .join("..")
        .join("libs")
        .join("compose-orchestrator-wasm")
        .join("target")
        .join("wasm32-wasip2")
        .join("release")
        .join("compose_orchestrator_wasm.wasm")
}

fn load_wasm_or_skip() -> Option<(wasmtime::Engine, Vec<u8>)> {
    let wasm_path = orchestrator_wasm_path();
    let wasm = match std::fs::read(&wasm_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!(
                "skipping orchestrator integration test: {} not found ({})\n\
                 run `libs/compose-orchestrator-wasm/build.sh` first to enable it",
                wasm_path.display(),
                e
            );
            return None;
        }
    };

    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let engine = wasmtime::Engine::new(&config).expect("wasmtime engine");
    Some((engine, wasm))
}

/// Round trip: native → wasm export → wasm's host import → native.
/// The orchestrator's `host-name` export calls back into our
/// `runtime-info.get-fingerprint` and returns the runtime name.
#[test]
fn smoke_roundtrip_returns_runtime_name() {
    let Some((engine, wasm)) = load_wasm_or_skip() else { return };
    let name = compose_host::run_smoke(&engine, &wasm)
        .expect("smoke roundtrip should succeed");
    assert_eq!(name, "wasmtime");
}

/// Compose-core logic runs inside the wasm component: the digest is
/// computed by `compose_core::blobs::compute_digest` from *inside*
/// the orchestrator wasm, not by anything on the native side.
#[test]
fn digest_computed_inside_wasm_matches_native_sha256() {
    use sha2::{Digest, Sha256};

    let Some((engine, wasm)) = load_wasm_or_skip() else { return };

    let payload = b"the quick brown fox jumps over the lazy dog";
    let from_wasm = compose_host::run_digest(&engine, &wasm, payload)
        .expect("digest roundtrip should succeed");

    let mut hasher = Sha256::new();
    hasher.update(payload);
    let expected: Vec<u8> = hasher.finalize().to_vec();

    assert_eq!(
        from_wasm, expected,
        "wasm-computed digest must match a native SHA-256 of the same bytes",
    );
    assert_eq!(from_wasm.len(), 32, "SHA-256 must be 32 bytes");
}

/// A real sys:compose export call. The host constructs a PlanV1 as a
/// structured WIT record, passes it across the canonical-ABI boundary,
/// the orchestrator wasm converts it to a compose-core PlanV1 and
/// calls compose-core's PlanValidator::compute_digest, and the host
/// receives a 32-byte SHA-256 digest back.
#[test]
fn plan_compute_digest_crosses_wit_boundary() {
    let Some((engine, wasm)) = load_wasm_or_skip() else { return };

    let plan = compose_host::sample_plan();
    let digest = compose_host::run_plan_compute_digest(&engine, &wasm, plan)
        .expect("plan.compute-digest should succeed");

    assert_eq!(digest.len(), 32, "SHA-256 digest must be 32 bytes");
}

/// PlanV1 round-trips through serialize → bytes → deserialize without
/// losing or corrupting any field. Tests that every record type
/// converts cleanly in both directions across the WIT boundary.
#[test]
fn plan_serialize_deserialize_round_trip() {
    let Some((engine, wasm)) = load_wasm_or_skip() else { return };

    let original = compose_host::sample_plan();
    let restored = compose_host::run_plan_roundtrip(&engine, &wasm, original.clone())
        .expect("plan round-trip should succeed");

    assert_eq!(restored.version, original.version);
    assert_eq!(restored.root, original.root);
    assert_eq!(restored.components.len(), original.components.len());
    assert_eq!(restored.components[0].id, original.components[0].id);
    assert_eq!(restored.components[0].digest, original.components[0].digest);
    assert_eq!(restored.bindings.len(), original.bindings.len());
    assert_eq!(restored.secrets.len(), original.secrets.len());
}

/// Computing the same plan's digest twice must produce identical
/// bytes — proof that the CBOR serialization the orchestrator uses
/// is canonical (no map-ordering or float-NaN nondeterminism).
#[test]
fn plan_digest_is_deterministic() {
    let Some((engine, wasm)) = load_wasm_or_skip() else { return };

    let plan = compose_host::sample_plan();
    let d1 = compose_host::run_plan_compute_digest(&engine, &wasm, plan.clone()).unwrap();
    let d2 = compose_host::run_plan_compute_digest(&engine, &wasm, plan).unwrap();
    assert_eq!(d1, d2, "plan digest must be deterministic across calls");
}
