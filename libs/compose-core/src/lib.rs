//! Wasmtime-free orchestrator core.
//!
//! This crate holds everything in the compositional WebAssembly system that
//! does not depend on a wasm runtime: plan validation, deterministic
//! composition, policy enforcement, trust orchestration, secret management,
//! events, metrics, audit, attestation, and the underlying content-addressable
//! blob store.
//!
//! Runtime glue (engine, store, linker, component instantiation, WASI wiring)
//! lives in the host crates that depend on this one.
pub mod attest;
pub mod audit;
pub mod blobs;
pub mod emit;
pub mod events;
pub mod limits;
pub mod metrics;
pub mod plan;
pub mod policy;
pub mod secrets;
pub mod trust;
pub mod types;

pub use attest::AttestationService;
pub use audit::AuditLogger;
pub use blobs::BlobStore;
pub use events::EventCollector;
pub use metrics::MetricsCollector;
pub use plan::PlanValidator;
pub use policy::{EnforcedPolicy, HostPolicy, PolicyEnforcer};
pub use secrets::SecretManager;
pub use types::*;
