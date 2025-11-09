/// Reference host implementation for sys:compose using Wasmtime
pub mod attest;
pub mod audit;
pub mod blobs;
pub mod emit;
pub mod events;
pub mod exec;
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
        let dev_backend = secrets::dev::DevBackend::new();
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
        let emit_handler = emit::EmitHandler::new(
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
