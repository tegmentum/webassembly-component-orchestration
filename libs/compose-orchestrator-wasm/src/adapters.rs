//! WIT-type ↔ compose-core-type adapters for the `sys:compose` exports.
//!
//! `wit-bindgen` generates one set of Rust types for the WIT records;
//! `compose-core` has its own serde-derived types. They have matching
//! field shapes but are different Rust types, so every export has to
//! convert at the boundary. The conversions are mechanical but
//! necessarily explicit — there is no derive macro that bridges the
//! two type systems today.
use crate::exports::sys::compose::plan::{
    ComponentSpec as WitComponentSpec, ImportBinding as WitImportBinding,
    PlanV1 as WitPlanV1, SecretBinding as WitSecretBinding,
};
use crate::sys::compose::types::{
    Capability as WitCapability, CapabilityLevel as WitCapabilityLevel,
    DeterminismMode as WitDeterminismMode, Error as WitError, ErrorCode as WitErrorCode,
    Policy as WitPolicy, ResourceLimits as WitResourceLimits,
};

use compose_core::types::{
    Capability as CoreCapability, CapabilityLevel as CoreCapabilityLevel,
    ComponentSpec as CoreComponentSpec, DeterminismMode as CoreDeterminismMode,
    Error as CoreError, ErrorCode as CoreErrorCode, ImportBinding as CoreImportBinding,
    Linkage as CoreLinkage,
    PlanV1 as CorePlanV1, Policy as CorePolicy, ResourceLimits as CoreResourceLimits,
    SecretBinding as CoreSecretBinding,
};

// ---------- WIT → compose-core ----------

pub fn wit_plan_to_core(p: WitPlanV1) -> CorePlanV1 {
    CorePlanV1 {
        version: p.version,
        root: p.root,
        components: p.components.into_iter().map(wit_component_to_core).collect(),
        bindings: p.bindings.into_iter().map(wit_binding_to_core).collect(),
        secrets: p.secrets.into_iter().map(wit_secret_to_core).collect(),
        policy: wit_policy_to_core(p.policy),
        // The WIT plan-v1 does not yet carry a linkage field; default to
        // Static (the historical behavior) until the WIT catches up.
        linkage: CoreLinkage::Static,
    }
}

fn wit_component_to_core(c: WitComponentSpec) -> CoreComponentSpec {
    CoreComponentSpec {
        id: c.id,
        digest: c.digest,
        source: c.source,
    }
}

fn wit_binding_to_core(b: WitImportBinding) -> CoreImportBinding {
    CoreImportBinding {
        // The WIT type does not yet carry consumer_id; compose-core's
        // does (as Option). Defaulting to None is the only honest
        // choice until the WIT catches up.
        consumer_id: None,
        import_name: b.import_name,
        provider_id: b.provider_id,
        export_name: b.export_name,
    }
}

fn wit_secret_to_core(s: WitSecretBinding) -> CoreSecretBinding {
    CoreSecretBinding {
        secret_id: s.secret_id,
        backend_uri: s.backend_uri,
    }
}

fn wit_policy_to_core(p: WitPolicy) -> CorePolicy {
    CorePolicy {
        determinism: match p.determinism {
            WitDeterminismMode::Strict => CoreDeterminismMode::Strict,
            WitDeterminismMode::Audit => CoreDeterminismMode::Audit,
            WitDeterminismMode::Relaxed => CoreDeterminismMode::Relaxed,
        },
        capabilities: p.capabilities.into_iter().map(wit_capability_to_core).collect(),
        tenant: p.tenant,
        limits: wit_limits_to_core(p.limits),
    }
}

fn wit_capability_to_core(c: WitCapability) -> CoreCapability {
    CoreCapability {
        name: c.name,
        level: match c.level {
            WitCapabilityLevel::Required => CoreCapabilityLevel::Required,
            WitCapabilityLevel::Optional => CoreCapabilityLevel::Optional,
        },
    }
}

fn wit_limits_to_core(l: WitResourceLimits) -> CoreResourceLimits {
    CoreResourceLimits {
        cpu_ms: l.cpu_ms,
        memory_bytes: l.memory_bytes,
        io_ops: l.io_ops,
    }
}

// ---------- compose-core → WIT ----------

pub fn core_plan_to_wit(p: CorePlanV1) -> WitPlanV1 {
    WitPlanV1 {
        version: p.version,
        root: p.root,
        components: p.components.into_iter().map(core_component_to_wit).collect(),
        bindings: p.bindings.into_iter().map(core_binding_to_wit).collect(),
        secrets: p.secrets.into_iter().map(core_secret_to_wit).collect(),
        policy: core_policy_to_wit(p.policy),
    }
}

fn core_component_to_wit(c: CoreComponentSpec) -> WitComponentSpec {
    WitComponentSpec {
        id: c.id,
        digest: c.digest,
        source: c.source,
    }
}

fn core_binding_to_wit(b: CoreImportBinding) -> WitImportBinding {
    // consumer_id is dropped because the WIT record has no slot for it.
    WitImportBinding {
        import_name: b.import_name,
        provider_id: b.provider_id,
        export_name: b.export_name,
    }
}

fn core_secret_to_wit(s: CoreSecretBinding) -> WitSecretBinding {
    WitSecretBinding {
        secret_id: s.secret_id,
        backend_uri: s.backend_uri,
    }
}

fn core_policy_to_wit(p: CorePolicy) -> WitPolicy {
    WitPolicy {
        determinism: match p.determinism {
            CoreDeterminismMode::Strict => WitDeterminismMode::Strict,
            CoreDeterminismMode::Audit => WitDeterminismMode::Audit,
            CoreDeterminismMode::Relaxed => WitDeterminismMode::Relaxed,
        },
        capabilities: p.capabilities.into_iter().map(core_capability_to_wit).collect(),
        tenant: p.tenant,
        limits: core_limits_to_wit(p.limits),
    }
}

fn core_capability_to_wit(c: CoreCapability) -> WitCapability {
    WitCapability {
        name: c.name,
        level: match c.level {
            CoreCapabilityLevel::Required => WitCapabilityLevel::Required,
            CoreCapabilityLevel::Optional => WitCapabilityLevel::Optional,
        },
    }
}

fn core_limits_to_wit(l: CoreResourceLimits) -> WitResourceLimits {
    WitResourceLimits {
        cpu_ms: l.cpu_ms,
        memory_bytes: l.memory_bytes,
        io_ops: l.io_ops,
    }
}

// ---------- error conversion ----------

pub fn core_err_to_wit(e: CoreError) -> WitError {
    WitError {
        code: core_code_to_wit(e.code),
        message: e.message,
        context: e.context,
    }
}

fn core_code_to_wit(c: CoreErrorCode) -> WitErrorCode {
    match c {
        CoreErrorCode::PlanInvalidSchema => WitErrorCode::PlanInvalidSchema,
        CoreErrorCode::PlanInvalidCbor => WitErrorCode::PlanInvalidCbor,
        CoreErrorCode::PlanMissingField => WitErrorCode::PlanMissingField,
        CoreErrorCode::PlanInvalidGraph => WitErrorCode::PlanInvalidGraph,
        CoreErrorCode::PlanCycleDetected => WitErrorCode::PlanCycleDetected,
        CoreErrorCode::EmitMissingBlob => WitErrorCode::EmitMissingBlob,
        CoreErrorCode::EmitInvalidDigest => WitErrorCode::EmitInvalidDigest,
        CoreErrorCode::EmitCompositionFailed => WitErrorCode::EmitCompositionFailed,
        CoreErrorCode::EmitLinkError => WitErrorCode::EmitLinkError,
        CoreErrorCode::ExecTrap => WitErrorCode::ExecTrap,
        CoreErrorCode::ExecTimeout => WitErrorCode::ExecTimeout,
        CoreErrorCode::ExecResourceExhausted => WitErrorCode::ExecResourceExhausted,
        CoreErrorCode::ExecCapabilityDenied => WitErrorCode::ExecCapabilityDenied,
        CoreErrorCode::ExecMissingExport => WitErrorCode::ExecMissingExport,
        CoreErrorCode::BlobNotFound => WitErrorCode::BlobNotFound,
        CoreErrorCode::BlobDigestMismatch => WitErrorCode::BlobDigestMismatch,
        CoreErrorCode::BlobIoError => WitErrorCode::BlobIoError,
        CoreErrorCode::TrustVerificationFailed => WitErrorCode::TrustVerificationFailed,
        CoreErrorCode::TrustSignatureInvalid => WitErrorCode::TrustSignatureInvalid,
        CoreErrorCode::TrustCertificateExpired => WitErrorCode::TrustCertificateExpired,
        CoreErrorCode::TrustUntrustedSource => WitErrorCode::TrustUntrustedSource,
        CoreErrorCode::SecretNotFound => WitErrorCode::SecretNotFound,
        CoreErrorCode::SecretAccessDenied => WitErrorCode::SecretAccessDenied,
        CoreErrorCode::SecretBackendError => WitErrorCode::SecretBackendError,
        CoreErrorCode::InvalidInput => WitErrorCode::InvalidInput,
        CoreErrorCode::InternalError => WitErrorCode::InternalError,
        CoreErrorCode::NotImplemented => WitErrorCode::NotImplemented,
        // PolicyViolation isn't in the WIT error-code variant yet;
        // fall back to ExecCapabilityDenied which is the closest match.
        CoreErrorCode::PolicyViolation => WitErrorCode::ExecCapabilityDenied,
    }
}
