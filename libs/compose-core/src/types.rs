/// Common types matching sys:compose WIT interfaces
use serde::{Deserialize, Serialize};

/// Content-addressed digest (SHA-256)
pub type Digest = Vec<u8>;

/// Opaque secret token handle
pub type SecretToken = String;

/// Tenant identifier for multi-tenancy
pub type TenantId = String;

/// Component or package identifier
pub type ComponentId = String;

/// Hierarchical error codes matching WIT
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    // Plan errors
    PlanInvalidSchema,
    PlanInvalidCbor,
    PlanMissingField,
    PlanInvalidGraph,
    PlanCycleDetected,

    // Emit errors
    EmitMissingBlob,
    EmitInvalidDigest,
    EmitCompositionFailed,
    EmitLinkError,

    // Exec errors
    ExecTrap,
    ExecTimeout,
    ExecResourceExhausted,
    ExecCapabilityDenied,
    ExecMissingExport,

    // Policy errors
    PolicyViolation,

    // Blob errors
    BlobNotFound,
    BlobDigestMismatch,
    BlobIoError,

    // Trust errors
    TrustVerificationFailed,
    TrustSignatureInvalid,
    TrustCertificateExpired,
    TrustUntrustedSource,

    // Secret errors
    SecretNotFound,
    SecretAccessDenied,
    SecretBackendError,

    // Generic errors
    InvalidInput,
    InternalError,
    NotImplemented,
}

/// Structured error with context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            context: None,
        }
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

/// Event severity levels
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EventLevel {
    Trace,
    Info,
    Warn,
    Error,
}

/// Structured event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub level: EventLevel,
    pub timestamp: u64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Resource limit specification
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_ops: Option<u64>,
}

/// Capability requirement level
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityLevel {
    Required,
    Optional,
}

/// Capability specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub name: String,
    pub level: CapabilityLevel,
}

/// Determinism mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeterminismMode {
    Strict,
    Audit,
    Relaxed,
}

/// How a plan's component imports are linked.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Linkage {
    /// Imports are merged into one sealed artifact at emit time
    /// (`wasm-compose`). This is the default and the historical behavior.
    #[default]
    Static,
    /// Imports are bound at exec time through the `compose:dynlink`
    /// host bridge (late binding). Requires non-Strict determinism.
    Runtime,
}

impl Linkage {
    /// Used by `serde(skip_serializing_if)` so the default (`Static`) is
    /// omitted from a plan's canonical encoding — keeping the digests of
    /// every pre-existing static plan byte-identical.
    pub fn is_static(&self) -> bool {
        matches!(self, Linkage::Static)
    }
}

/// Policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub determinism: DeterminismMode,
    pub capabilities: Vec<Capability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<TenantId>,
    pub limits: ResourceLimits,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            determinism: DeterminismMode::Relaxed,
            capabilities: Vec::new(),
            tenant: None,
            limits: ResourceLimits::default(),
        }
    }
}

/// Component specification in the plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSpec {
    pub id: ComponentId,
    pub digest: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Import binding specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportBinding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer_id: Option<ComponentId>,
    pub import_name: String,
    pub provider_id: ComponentId,
    pub export_name: String,
}

/// Secret binding specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretBinding {
    pub secret_id: String,
    pub backend_uri: String,
}

/// Explicit re-export of a non-root plug's interface.
///
/// The composer auto-exports the socket (root) component's exports.
/// `ExplicitExport` entries surface additional interfaces from a
/// named plug at the composed component's outer world — required
/// when a host needs to call a plug's export directly (e.g.
/// `sqlink:wasm/dispatch-bridge@0.1.0` on the sqlite-lib plug)
/// rather than through the root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExplicitExport {
    /// Component id (in the plan's components list) whose export is
    /// being surfaced.
    pub source_instance: ComponentId,
    /// Fully qualified WIT interface name (e.g.
    /// `sqlite:extension/types@0.1.0`).
    pub interface_name: String,
}

/// Composition plan (version 1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanV1 {
    pub version: String,
    pub root: ComponentId,
    pub components: Vec<ComponentSpec>,
    pub bindings: Vec<ImportBinding>,
    pub secrets: Vec<SecretBinding>,
    pub policy: Policy,
    /// Linking strategy. Omitted from the canonical encoding when
    /// `Static` (the default), so static plans keep their existing digests.
    #[serde(default, skip_serializing_if = "Linkage::is_static")]
    pub linkage: Linkage,
    /// Additional non-root exports to surface at the composed
    /// component's outer world. Omitted from the canonical encoding
    /// when empty, so plans that predate this field keep their
    /// existing digests byte-for-byte.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explicit_exports: Vec<ExplicitExport>,
}

/// Composition result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionResult {
    pub digest: Digest,
    pub size: u64,
    pub emit_key: Digest,
}

/// Export metadata for reflection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportInfo {
    pub name: String,
    pub type_sig: String,
}

/// Execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub exit_code: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exec_key: Digest,
}

/// HTTP request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// HTTP response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u32,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Verification metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationMetadata {
    pub signer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    pub backend: String,
}

/// Verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub verified: bool,
    pub metadata: VerificationMetadata,
}
