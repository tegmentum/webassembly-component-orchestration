//! Orchestrator-as-wasm: compose:host imports + sys:compose exports.
//!
//! Today this crate exports:
//!
//! - `compose:host/smoke` (test-only, will be removed)
//! - `sys:compose/plan@1.0.0` — `serialize`, `deserialize`,
//!   `compute-digest`, and `validate`.
//! - `sys:compose/blobs@1.0.0` — `put`, `get`, `has`, `size`,
//!   `delete`, `list-all`. Thin bridge over
//!   `compose_core::blobs::BlobStore` at [`BLOBS_DIR`].
//! - `sys:compose/emit@1.0.0` — `compose`, `get-artifact`,
//!   `check-cache`. Bridges to
//!   `compose_core::emit::EmitHandler` over [`BLOBS_DIR`] +
//!   [`EMIT_CACHE_DIR`].
//! - `sys:compose/trust@1.0.0` — `verify`, `verify-digest`,
//!   `is-trusted`, `trust-digest`, `untrust-digest`. Bridges to
//!   `compose_core::trust::TrustStore` at [`TRUST_DIR`].
//! - `sys:compose/rdf@1.0.0` — `plan-to-turtle`,
//!   `plan-to-turtle-with-iri`, `plan-to-turtle-with-artifact`,
//!   `plan-from-turtle`, `plan-from-turtle-with-iri`. Decodes canonical
//!   CBOR plan bytes (via `compose_core::plan::deserialize`) and hands
//!   the result to the matching `compose_rdf` writer; the
//!   with-artifact variant additionally emits comp:hasArtifact +
//!   optional comp:compositionDigest anchors. Pure computation; no
//!   preopens required.
//!
//! Not yet exported: `sys:compose/exec@1.0.0` (needs a wasm runtime
//! inside the guest) and `sys:compose/events@1.0.0` (the compose-core
//! `EventCollector` fits the shape but stays local to this component
//! today; wiring it as an export would round-trip events through the
//! host to no benefit until a host-side sink is defined).
//!
//! ## Preopen contract
//!
//! Every subsystem that touches persistent state opens its store at
//! a fixed guest-side path. The host MUST preopen each of these via
//! `wasi:filesystem` when instantiating this component. Host-side
//! mount points are at the host's discretion (typically
//! `.compose/{blobs,emit-cache,trust}/` from `HostConfig`); from
//! the wasm side they are always at the paths below.
//!
//! - [`BLOBS_DIR`] — content-addressed blob storage. Required by
//!   `plan.validate`, every `blobs.*` export, and every `emit.*`
//!   export.
//! - [`EMIT_CACHE_DIR`] — emit-key → composed-digest cache. Required
//!   by every `emit.*` export.
//! - [`TRUST_DIR`] — persistent trust metadata. Required by every
//!   `trust.*` export.
//!
//! If a required preopen is missing, the corresponding call returns
//! an `internal-error` rather than silently pretending the store is
//! empty.
wit_bindgen::generate!({
    path: "wit",
    world: "orchestrator",
    generate_all,
});

mod adapters;
mod wit_secure_log;

use std::sync::{Arc, Mutex};

use exports::compose::host::smoke::Guest as SmokeGuest;
use exports::sys::compose::blobs::Guest as BlobsGuest;
use exports::sys::compose::emit::{
    CompositionResult as WitCompositionResult, Guest as EmitGuest,
};
use exports::sys::compose::plan::{Guest as PlanGuest, PlanV1 as WitPlanV1};
use exports::sys::compose::rdf::Guest as RdfGuest;
use exports::sys::compose::trust::{
    Guest as TrustGuest, VerificationMetadata as WitVerificationMetadata,
    VerificationResult as WitVerificationResult,
};
use sys::compose::types::{Error as WitError, ErrorCode as WitErrorCode};

/// Guest-side path the host MUST preopen for blob storage. See module
/// docs for the full preopen contract.
const BLOBS_DIR: &str = "/blobs";

/// Guest-side path the host MUST preopen for the emit-key cache. See
/// module docs for the full preopen contract.
const EMIT_CACHE_DIR: &str = "/emit-cache";

/// Guest-side path the host MUST preopen for trust metadata. See
/// module docs for the full preopen contract.
const TRUST_DIR: &str = "/trust";

/// Environment variable a host can set to raise the maximum blob size
/// this guest will accept during `validate`. The value is parsed as a
/// base-10 `u64` byte count. If unset or unparseable, the guest falls
/// back to [`compose_core::limits::DEFAULT_MAX_BLOB_SIZE`] (100 MiB) —
/// the safe multi-tenant hedge.
///
/// Build-tool hosts (composectl, sqlink, ducklink, datafission) that
/// need to validate plans referencing composed runtimes larger than
/// 100 MiB (e.g. postgis-composed.wasm ~112 MiB) should set this to
/// [`compose_core::limits::BUILD_TOOL_MAX_BLOB_SIZE`] (1 GiB) or larger
/// via `wasi:cli/environment` before instantiating this component.
const MAX_BLOB_SIZE_ENV: &str = "COMPOSE_MAX_BLOB_SIZE";

/// Resolve the maximum blob size for the current invocation from the
/// [`MAX_BLOB_SIZE_ENV`] environment variable, falling back to the
/// portable core's `DEFAULT_MAX_BLOB_SIZE` when unset. Called per
/// `validate` invocation so the host can retune between calls without
/// re-instantiating the guest.
fn resolve_max_blob_size() -> u64 {
    std::env::var(MAX_BLOB_SIZE_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(compose_core::limits::DEFAULT_MAX_BLOB_SIZE)
}

/// Open the compose-core `BlobStore` rooted at [`BLOBS_DIR`]. Surfaces
/// a missing wasi:filesystem preopen as an `internal-error` at the WIT
/// boundary rather than papering over it.
fn open_blob_store() -> Result<compose_core::blobs::BlobStore, WitError> {
    compose_core::blobs::BlobStore::new(
        std::path::PathBuf::from(BLOBS_DIR),
        resolve_max_blob_size(),
    )
    .map_err(|e| WitError {
        code: WitErrorCode::InternalError,
        message: format!(
            "failed to open blob store at {BLOBS_DIR} \
             (did the host preopen it via wasi:filesystem?): {e}"
        ),
        context: None,
    })
}

/// Construct an `EmitHandler` over the guest-side preopens for blobs
/// and the emit cache. The `EventCollector` is process-local; nothing
/// today ships those events outside the guest.
fn open_emit_handler() -> Result<compose_core::emit::EmitHandler, WitError> {
    let blobs = open_blob_store()?;
    let events = compose_core::EventCollector::default();
    Ok(compose_core::emit::EmitHandler::new(
        blobs,
        events,
        std::path::PathBuf::from(EMIT_CACHE_DIR),
    ))
}

/// Open the compose-core `TrustStore` rooted at [`TRUST_DIR`]. Same
/// preopen-missing story as [`open_blob_store`].
fn open_trust_store() -> Result<compose_core::trust::TrustStore, WitError> {
    compose_core::trust::TrustStore::new(
        std::path::PathBuf::from(TRUST_DIR),
        compose_core::SystemClock::shared(),
    )
    .map_err(|e| WitError {
        code: WitErrorCode::InternalError,
        message: format!(
            "failed to open trust store at {TRUST_DIR} \
             (did the host preopen it via wasi:filesystem?): {e}"
        ),
        context: None,
    })
}

/// The exported component. Bind every interface this crate provides
/// onto this one struct; wit-bindgen wires them up via `export!`.
struct Component;

impl SmokeGuest for Component {
    fn host_name() -> String {
        let fp = crate::compose::host::runtime_info::get_fingerprint();
        fp.runtime_name
    }

    fn digest(bytes: Vec<u8>) -> Vec<u8> {
        compose_core::blobs::compute_digest(&bytes)
    }

    fn tegmentum_probe() -> Result<String, String> {
        // Touch host:bootstrap.args. The import surfaces to Rust as
        // `crate::host::bootstrap::bootstrap::args()`. A no-op host
        // (like wasmtime-cli's trap-unknown-imports path) will trap
        // here — treat that as a diagnostic error, not a crash.
        let args = crate::host::bootstrap::bootstrap::args();
        let n_args = args.len();

        // Touch tegmentum:runtime/control by constructing a
        // Runtime resource with default (all-unbounded) limits.
        // If no host provides tegmentum:runtime the constructor's
        // canonical-ABI call traps; wit-bindgen returns via panic,
        // which would abort. So we deliberately do NOT invoke the
        // constructor here at runtime — instead the reference below
        // takes a function-pointer view of the import, which is
        // enough for LLVM's `#[used]`-like retention semantics
        // (function pointer used in reachable code path keeps the
        // extern reference alive) without a runtime trap.
        //
        // If a future embedder DOES provide tegmentum:runtime, this
        // probe can be extended to actually call `Runtime::new` and
        // report success. Today we just report that the reference
        // is live.
        let rt_ctor_probe: fn(
            crate::tegmentum::runtime::types::ResourceLimits,
        ) -> crate::tegmentum::runtime::control::Runtime =
            crate::tegmentum::runtime::control::Runtime::new;
        // Prevent LLVM from optimizing the reference away.
        core::hint::black_box(rt_ctor_probe as usize);

        Ok(format!("args={n_args} runtime=probed"))
    }

    fn audit_demo(tenant: String, count: u32) -> Result<u64, String> {
        // Back compose-core's AuditLogger with the imported secure-log
        // component. This is the same AuditLogger that runs natively
        // over SQLite — here it runs over a WIT-imported, wac-composed
        // secure-log backend, inside the wasm sandbox.
        let backend = wit_secure_log::WitSecureLog::open(":memory:")?;
        let shared: compose_core::SharedSecureLog = Arc::new(Mutex::new(backend));
        let logger = compose_core::AuditLogger::new(
            shared.clone(),
            compose_core::SystemClock::shared(),
        );

        let digest = vec![0u8; 32];
        for _ in 0..count {
            logger
                .log_exec(&digest, &digest, Some(&tenant), "success", Some(0))
                .map_err(|e| e.to_string())?;
        }

        // Verify the tenant's hash chain end to end, then report the
        // head sequence number as proof of how many entries landed.
        logger
            .verify_tenant(Some(&tenant))
            .map_err(|e| e.to_string())?;
        let head = shared
            .lock()
            .map_err(|_| "secure log mutex poisoned".to_string())?
            .head(&tenant)
            .map_err(|e| e.to_string())?
            .unwrap_or(0);
        Ok(head)
    }
}

impl PlanGuest for Component {
    fn serialize(plan: WitPlanV1) -> Result<Vec<u8>, WitError> {
        let core_plan = adapters::wit_plan_to_core(plan);
        compose_core::plan::serialize(&core_plan).map_err(adapters::core_err_to_wit)
    }

    fn deserialize(bytes: Vec<u8>) -> Result<WitPlanV1, WitError> {
        let core_plan =
            compose_core::plan::deserialize(&bytes).map_err(adapters::core_err_to_wit)?;
        Ok(adapters::core_plan_to_wit(core_plan))
    }

    fn validate(plan: WitPlanV1) -> Result<(), WitError> {
        let core_plan = adapters::wit_plan_to_core(plan);
        let blobs = open_blob_store()?;
        compose_core::PlanValidator::new(blobs)
            .validate(&core_plan)
            .map_err(adapters::core_err_to_wit)
    }

    fn compute_digest(plan: WitPlanV1) -> Result<Vec<u8>, WitError> {
        let core_plan = adapters::wit_plan_to_core(plan);
        compose_core::plan::compute_plan_digest(&core_plan).map_err(adapters::core_err_to_wit)
    }
}

impl BlobsGuest for Component {
    fn put(bytes: Vec<u8>) -> Result<Vec<u8>, WitError> {
        open_blob_store()?
            .put(&bytes)
            .map_err(adapters::core_err_to_wit)
    }

    fn get(digest: Vec<u8>) -> Result<Vec<u8>, WitError> {
        open_blob_store()?
            .get(&digest)
            .map_err(adapters::core_err_to_wit)
    }

    fn has(digest: Vec<u8>) -> bool {
        // `has` returns `bool` in the WIT — a missing preopen collapses
        // to "no such blob", matching the same signal a missing file
        // would give.
        match open_blob_store() {
            Ok(store) => store.has(&digest),
            Err(_) => false,
        }
    }

    fn size(digest: Vec<u8>) -> Option<u64> {
        // Same collapse as `has` — no way to signal "store unreachable"
        // through an `option<u64>` return.
        match open_blob_store() {
            Ok(store) => store.size(&digest),
            Err(_) => None,
        }
    }

    fn delete(digest: Vec<u8>) -> Result<(), WitError> {
        open_blob_store()?
            .delete(&digest)
            .map_err(adapters::core_err_to_wit)
    }

    fn list_all() -> Vec<Vec<u8>> {
        // `list-all` returns `list<digest>` — no error channel. An
        // unreachable store surfaces as an empty list, same as an
        // empty store.
        match open_blob_store() {
            Ok(store) => store.list_all(),
            Err(_) => Vec::new(),
        }
    }
}

impl EmitGuest for Component {
    fn compose(plan: WitPlanV1) -> Result<WitCompositionResult, WitError> {
        let core_plan = adapters::wit_plan_to_core(plan);
        let handler = open_emit_handler()?;
        handler
            .compose(&core_plan)
            .map(adapters::core_composition_to_wit)
            .map_err(adapters::core_err_to_wit)
    }

    fn get_artifact(digest: Vec<u8>) -> Result<Vec<u8>, WitError> {
        let handler = open_emit_handler()?;
        handler
            .get_artifact(&digest)
            .map_err(adapters::core_err_to_wit)
    }

    fn check_cache(emit_key: Vec<u8>) -> Option<Vec<u8>> {
        // Cache-miss on unreachable store — same reasoning as
        // `blobs::has`.
        match open_emit_handler() {
            Ok(handler) => handler.check_cache(&emit_key),
            Err(_) => None,
        }
    }
}

impl RdfGuest for Component {
    fn plan_to_turtle(plan_cbor: Vec<u8>) -> Result<String, WitError> {
        let plan = compose_core::plan::deserialize(&plan_cbor)
            .map_err(adapters::core_err_to_wit)?;
        Ok(compose_rdf::plan_to_turtle(&plan))
    }

    fn plan_to_turtle_with_iri(
        plan_cbor: Vec<u8>,
        plan_iri: String,
    ) -> Result<String, WitError> {
        let plan = compose_core::plan::deserialize(&plan_cbor)
            .map_err(adapters::core_err_to_wit)?;
        Ok(compose_rdf::plan_to_turtle_with_iri(&plan, &plan_iri))
    }

    fn plan_to_turtle_with_artifact(
        plan_cbor: Vec<u8>,
        plan_iri: String,
        artifact_url: String,
        digest_hex: Option<String>,
    ) -> Result<String, WitError> {
        let plan = compose_core::plan::deserialize(&plan_cbor)
            .map_err(adapters::core_err_to_wit)?;
        Ok(compose_rdf::plan_to_turtle_with_artifact(
            &plan,
            &plan_iri,
            &artifact_url,
            digest_hex.as_deref(),
        ))
    }

    fn plan_from_turtle(turtle: String) -> Result<Vec<u8>, WitError> {
        let plan = compose_rdf::plan_from_turtle(&turtle).map_err(|e| WitError {
            code: WitErrorCode::InvalidInput,
            message: format!("plan-from-turtle: {e:#}"),
            context: None,
        })?;
        compose_core::plan::serialize(&plan).map_err(adapters::core_err_to_wit)
    }

    fn plan_from_turtle_with_iri(
        turtle: String,
        plan_iri: String,
    ) -> Result<Vec<u8>, WitError> {
        let plan = compose_rdf::plan_from_turtle_with_iri(&turtle, &plan_iri).map_err(|e| {
            WitError {
                code: WitErrorCode::InvalidInput,
                message: format!("plan-from-turtle-with-iri: {e:#}"),
                context: None,
            }
        })?;
        compose_core::plan::serialize(&plan).map_err(adapters::core_err_to_wit)
    }
}

impl TrustGuest for Component {
    fn verify(
        digest: Vec<u8>,
        bytes: Vec<u8>,
        signature: Option<Vec<u8>>,
    ) -> Result<WitVerificationResult, WitError> {
        let store = open_trust_store()?;
        store
            .verify(&digest, &bytes, signature.as_deref())
            .map(adapters::core_verification_result_to_wit)
            .map_err(adapters::core_err_to_wit)
    }

    fn verify_digest(digest: Vec<u8>) -> Result<WitVerificationResult, WitError> {
        let store = open_trust_store()?;
        store
            .verify_digest(&digest)
            .map(adapters::core_verification_result_to_wit)
            .map_err(adapters::core_err_to_wit)
    }

    fn is_trusted(digest: Vec<u8>) -> bool {
        match open_trust_store() {
            Ok(store) => store.is_trusted(&digest),
            Err(_) => false,
        }
    }

    fn trust_digest(
        digest: Vec<u8>,
        metadata: WitVerificationMetadata,
    ) -> Result<(), WitError> {
        let store = open_trust_store()?;
        store
            .trust_digest(&digest, adapters::wit_verification_metadata_to_core(metadata))
            .map_err(adapters::core_err_to_wit)
    }

    fn untrust_digest(digest: Vec<u8>) -> Result<(), WitError> {
        let store = open_trust_store()?;
        store
            .untrust_digest(&digest)
            .map_err(adapters::core_err_to_wit)
    }
}

export!(Component);
