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
pub mod host;
pub mod limits;
pub mod metrics;
pub mod plan;
pub mod policy;
pub mod secrets;
pub mod trust;
pub mod types;

pub use attest::AttestationService;
pub use audit::{AuditLogger, SharedSecureLog};
pub use blobs::BlobStore;
// Re-export the secure-log surface host crates need to construct a
// backend without taking their own direct dependency on the crate.
pub use events::EventCollector;
pub use host::{Clock, SharedClock, SystemClock};
pub use metrics::MetricsCollector;
pub use plan::PlanValidator;
pub use policy::{EnforcedPolicy, HostPolicy, PolicyEnforcer};
pub use secrets::SecretManager;
pub use secure_log;
pub use types::*;
