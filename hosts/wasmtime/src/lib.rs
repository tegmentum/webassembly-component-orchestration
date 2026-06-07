//! Reference host implementation for `sys:compose` on Wasmtime.
//!
//! The portable orchestrator logic (plans, composition, policy, trust,
//! secrets, audit, attestation, metrics, events, blob CAS) lives in the
//! [`compose_core`] crate. This crate adds the wasmtime-specific runtime
//! glue — engine setup, component instantiation via `exec` — plus the
//! pkcs11 secret backend, which depends on the wasmtime-backed adapter.
pub mod cbor_val;
pub mod compose_host;
pub mod dynlink;
pub mod exec;
#[cfg(feature = "pkcs11")]
pub mod pkcs11_backend;
pub mod pkcs11_signer;

pub use pkcs11_signer::{Pkcs11Signer, Pkcs11SignerConfig};

// Re-export the portable core so downstream consumers
// (composectl, conformance/runner) keep working unchanged.
pub use compose_core::types::*;
pub use compose_core::{
    attest, audit, blobs, emit, events, host, limits, metrics, plan, policy, secrets, trust, types,
};
pub use compose_core::{
    AttestationService, AuditLogger, BlobStore, EnforcedPolicy, EventCollector, HostPolicy,
    MetricsCollector, PlanValidator, PolicyEnforcer, SecretManager, SharedClock, SystemClock,
};

use anyhow::Result;
use std::path::PathBuf;
use wasmtime::Engine;

/// Fixed ed25519 seed for the development attestation key. Dev/CI only —
/// production must supply a PKCS#11 / HSM / TPM-backed signer instead.
const DEV_ATTEST_SEED: [u8; 32] = *b"compose-host-dev-attest-seed!!!\0";

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
    /// Optional PKCS#11-backed attestation signer. When set, attestations
    /// are signed by a key inside the composed `keys:keystore` (softhsm)
    /// component instead of the in-process dev key. `None` keeps the
    /// software signer (dev/CI default).
    pub attest_pkcs11: Option<pkcs11_signer::Pkcs11SignerConfig>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            blob_dir: PathBuf::from(".compose/blobs"),
            cache_dir: PathBuf::from(".compose/cache"),
            trust_dir: PathBuf::from(".compose/trust"),
            audit_dir: PathBuf::from(".compose/audit"),
            max_blob_size: 100 * 1024 * 1024, // 100 MB
            attest_pkcs11: None,
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
    pub clock: SharedClock,
    /// Trust store gating which component digests may be linked at runtime.
    pub trust: compose_core::trust::TrustStore,
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

        // The host satisfies the orchestrator's clock capability using a
        // std-based implementation. Filesystem (the blob CAS, audit log,
        // emit cache, trust metadata) lowers to wasi:filesystem when the
        // orchestrator runs as wasm — no Rust-level abstraction needed.
        let clock: SharedClock = SystemClock::shared();
        let blobs = BlobStore::new(config.blob_dir.clone(), config.max_blob_size)?;

        let events = EventCollector::new(clock.clone());
        let secrets = SecretManager::new(clock.clone());
        let trust = compose_core::trust::TrustStore::new(config.trust_dir.clone(), clock.clone())?;

        // Register dev backend by default with some test secrets
        let dev_backend = compose_core::secrets::dev::DevBackend::new(clock.clone());
        // Add test secrets for demos
        dev_backend.add_secret("api-key", b"super-secret-key-12345");
        dev_backend.add_secret("db-password", b"p@ssw0rd!");
        secrets.register_backend(Box::new(dev_backend))?;

        let policy_enforcer = PolicyEnforcer::with_defaults();

        // Durable, tamper-evident audit log backed by SQLite. Each
        // tenant becomes a hash-chained secure-log stream; entries
        // cannot be altered or deleted without breaking verify_chain.
        let audit_db = config.audit_dir.join("audit.db");
        let audit_store =
            secure_log_sqlite::SqliteSecureLogStore::open(&audit_db).map_err(|e| {
                anyhow::anyhow!("failed to open audit log at {}: {e}", audit_db.display())
            })?;
        let secure_log = secure_log::NativeSecureLog::new(
            Box::new(audit_store),
            Box::new(secure_log::CborEncoder::new()),
        );
        let audit_logger = AuditLogger::new(
            std::sync::Arc::new(std::sync::Mutex::new(secure_log)),
            clock.clone(),
        );
        let metrics = MetricsCollector::new(clock.clone());

        // Attestation signing capability. The default is an in-process
        // ed25519 key derived from a fixed dev seed — fine for dev and
        // CI, NOT for production, where a PKCS#11 / HSM / TPM-backed
        // signer (keeping the private key off-host) should be wired in.
        // See compose_core::host::Signer and the pkcs11 feature.
        // Attestation signing key. Default is the in-process dev seed
        // (dev/CI only). If a PKCS#11 signer is configured, the key lives
        // in the composed keys:keystore (softhsm) component and never
        // leaves the sandbox — the production-shaped path.
        let signer = match &config.attest_pkcs11 {
            Some(pk) => pkcs11_signer::Pkcs11Signer::shared(&engine, pk)
                .map_err(|e| anyhow::anyhow!("failed to open PKCS#11 attestation signer: {e}"))?,
            None => compose_core::host::SoftwareSigner::shared(DEV_ATTEST_SEED),
        };
        let attestation =
            AttestationService::new("wasmtime-host".to_string(), signer, clock.clone());

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
            clock,
            trust,
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
            self.trust.clone(),
        )
    }
}
