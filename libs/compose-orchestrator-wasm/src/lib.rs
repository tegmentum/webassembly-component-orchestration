//! Orchestrator-as-wasm: compose:host imports + sys:compose exports.
//!
//! Today this crate exports:
//!
//! - `compose:host/smoke` (test-only, will be removed)
//! - `sys:compose/plan@1.0.0` — `serialize`, `deserialize`,
//!   `compute-digest`, and `validate`.
//!
//! ## Preopen contract
//!
//! `validate` needs to check that every component digest in the plan
//! has a corresponding blob present. It does that by opening a
//! `compose_core::blobs::BlobStore` rooted at [`BLOBS_DIR`], a
//! guest-side path the host MUST preopen via `wasi:filesystem` when
//! it instantiates this component. If the preopen is missing,
//! `validate` returns an `internal-error` rather than silently
//! pretending no blobs exist.
//!
//! The host-side mount-point is at the host's discretion (typically
//! `.compose/blobs/` from `HostConfig`); from the wasm side it is
//! always `/blobs`.
wit_bindgen::generate!({
    path: "wit",
    world: "orchestrator",
    generate_all,
});

mod adapters;
mod wit_secure_log;

use std::sync::{Arc, Mutex};

use exports::compose::host::smoke::Guest as SmokeGuest;
use exports::sys::compose::plan::{Guest as PlanGuest, PlanV1 as WitPlanV1};
use sys::compose::types::Error as WitError;

/// Guest-side path the host MUST preopen for blob storage. See module
/// docs for the full preopen contract.
const BLOBS_DIR: &str = "/blobs";

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
        let blobs = compose_core::blobs::BlobStore::new(
            std::path::PathBuf::from(BLOBS_DIR),
            resolve_max_blob_size(),
        )
        .map_err(|e| WitError {
            code: sys::compose::types::ErrorCode::InternalError,
            message: format!(
                "failed to open blob store at {BLOBS_DIR} \
                 (did the host preopen it via wasi:filesystem?): {e}"
            ),
            context: None,
        })?;
        compose_core::PlanValidator::new(blobs)
            .validate(&core_plan)
            .map_err(adapters::core_err_to_wit)
    }

    fn compute_digest(plan: WitPlanV1) -> Result<Vec<u8>, WitError> {
        let core_plan = adapters::wit_plan_to_core(plan);
        compose_core::plan::compute_plan_digest(&core_plan).map_err(adapters::core_err_to_wit)
    }
}

export!(Component);
