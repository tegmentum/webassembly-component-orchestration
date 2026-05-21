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

#[test]
fn smoke_roundtrip_returns_runtime_name() {
    let wasm_path = orchestrator_wasm_path();
    let wasm = match std::fs::read(&wasm_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!(
                "skipping orchestrator smoke test: {} not found ({})\n\
                 run `libs/compose-orchestrator-wasm/build.sh` first to enable it",
                wasm_path.display(),
                e
            );
            return;
        }
    };

    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let engine = wasmtime::Engine::new(&config).expect("wasmtime engine");

    let name = compose_host::run_smoke(&engine, &wasm)
        .expect("smoke roundtrip should succeed");

    assert_eq!(name, "wasmtime");
}
