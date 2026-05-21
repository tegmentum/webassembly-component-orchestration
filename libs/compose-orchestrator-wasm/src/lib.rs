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

use exports::compose::host::smoke::Guest as SmokeGuest;
use exports::sys::compose::plan::{Guest as PlanGuest, PlanV1 as WitPlanV1};
use sys::compose::types::Error as WitError;

/// Guest-side path the host MUST preopen for blob storage. See module
/// docs for the full preopen contract.
const BLOBS_DIR: &str = "/blobs";

/// Default maximum blob size: 100 MiB. Matches the host's default
/// HostConfig.max_blob_size. A future revision may surface this via
/// configuration rather than hardcoding it on both sides.
const MAX_BLOB_SIZE: u64 = 100 * 1024 * 1024;

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
            MAX_BLOB_SIZE,
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
