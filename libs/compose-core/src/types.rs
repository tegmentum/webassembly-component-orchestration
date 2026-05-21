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

/// Composition plan (version 1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanV1 {
    pub version: String,
    pub root: ComponentId,
    pub components: Vec<ComponentSpec>,
    pub bindings: Vec<ImportBinding>,
    pub secrets: Vec<SecretBinding>,
    pub policy: Policy,
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
