//! Reference host implementation for `sys:compose` on Wasmtime.
//!
//! The portable orchestrator logic (plans, composition, policy, trust,
//! secrets, audit, attestation, metrics, events, blob CAS) lives in the
//! [`compose_core`] crate. This crate adds the wasmtime-specific runtime
//! glue — engine setup, component instantiation via `exec` — plus the
//! pkcs11 secret backend, which depends on the wasmtime-backed adapter.
pub mod exec;
#[cfg(feature = "pkcs11")]
pub mod pkcs11_backend;

// Re-export the portable core so downstream consumers
// (composectl, conformance/runner) keep working unchanged.
pub use compose_core::{
    attest, audit, blobs, emit, events, limits, metrics, plan, policy, secrets, trust, types,
};
pub use compose_core::{
    AttestationService, AuditLogger, BlobStore, EnforcedPolicy, EventCollector, HostPolicy,
    MetricsCollector, PlanValidator, PolicyEnforcer, SecretManager,
};
pub use compose_core::types::*;

use anyhow::Result;
use std::path::PathBuf;
use wasmtime::Engine;

/// Compositor host configuration
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Path to blob storage directory
    pub blob_dir: PathBuf,
    /// Path to cache directory
    pub cache_dir: PathBuf,
    /// Path to trust store directory
    pub trust_dir: PathBuf,
    /// Path to audit log directory
    pub audit_dir: PathBuf,
    /// Maximum blob size in bytes
    pub max_blob_size: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            blob_dir: PathBuf::from(".compose/blobs"),
            cache_dir: PathBuf::from(".compose/cache"),
            trust_dir: PathBuf::from(".compose/trust"),
            audit_dir: PathBuf::from(".compose/audit"),
            max_blob_size: 100 * 1024 * 1024, // 100 MB
        }
    }
}

/// Main compositor host runtime
pub struct CompositorHost {
    pub config: HostConfig,
    pub engine: Engine,
    pub blobs: BlobStore,
    pub events: EventCollector,
    pub secrets: SecretManager,
    pub policy_enforcer: PolicyEnforcer,
    pub audit_logger: AuditLogger,
    pub metrics: MetricsCollector,
    pub attestation: AttestationService,
}

impl CompositorHost {
    /// Create a new compositor host with the given configuration
    pub fn new(config: HostConfig) -> Result<Self> {
        // Create directories
        std::fs::create_dir_all(&config.blob_dir)?;
        std::fs::create_dir_all(&config.cache_dir)?;
        std::fs::create_dir_all(&config.trust_dir)?;
        std::fs::create_dir_all(&config.audit_dir)?;

        // Configure Wasmtime engine
        let mut wasmtime_config = wasmtime::Config::new();
        wasmtime_config.wasm_component_model(true);
        let engine = Engine::new(&wasmtime_config)?;

        // Initialize subsystems
        let blobs = BlobStore::new(config.blob_dir.clone(), config.max_blob_size)?;
        let events = EventCollector::new();
        let secrets = SecretManager::new();

        // Register dev backend by default with some test secrets
        let dev_backend = compose_core::secrets::dev::DevBackend::new();
        // Add test secrets for demos
        dev_backend.add_secret("api-key", b"super-secret-key-12345");
        dev_backend.add_secret("db-password", b"p@ssw0rd!");
        secrets.register_backend(Box::new(dev_backend))?;

        // Initialize policy enforcer with default host policy
        let policy_enforcer = PolicyEnforcer::with_defaults();

        // Initialize audit logger
        let audit_logger = AuditLogger::new(config.audit_dir.clone())?;

        // Initialize metrics collector
        let metrics = MetricsCollector::new();

        // Initialize attestation service
        let attestation = AttestationService::new("wasmtime-host".to_string());

        Ok(Self {
            config,
            engine,
            blobs,
            events,
            secrets,
            policy_enforcer,
            audit_logger,
            metrics,
            attestation,
        })
    }

    /// Get a plan validator for this host
    pub fn plan_validator(&self) -> PlanValidator {
        PlanValidator::new(self.blobs.clone())
    }

    /// Get an exec handler for this host
    pub fn exec_handler(&self) -> exec::ExecHandler {
        let emit_handler = compose_core::emit::EmitHandler::new(
            self.blobs.clone(),
            self.events.clone(),
            self.config.cache_dir.clone(),
        );

        exec::ExecHandler::new(
            self.engine.clone(),
            self.blobs.clone(),
            emit_handler,
            self.events.clone(),
            self.config.cache_dir.clone(),
            self.policy_enforcer.clone(),
            self.audit_logger.clone(),
            self.metrics.clone(),
        )
    }
}
