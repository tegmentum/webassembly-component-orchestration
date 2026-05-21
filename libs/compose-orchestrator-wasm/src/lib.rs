//! Orchestrator-as-wasm: compose:host imports + sys:compose exports.
//!
//! Today this crate exports:
//!
//! - `compose:host/smoke` (test-only, will be removed)
//! - `sys:compose/plan@1.0.0` — `serialize`, `deserialize`, `compute-digest`.
//!   `validate` is stubbed pending filesystem-backed blob storage via
//!   wasi:filesystem preopens.
wit_bindgen::generate!({
    path: "wit",
    world: "orchestrator",
    generate_all,
});

mod adapters;

use exports::compose::host::smoke::Guest as SmokeGuest;
use exports::sys::compose::plan::{Guest as PlanGuest, PlanV1 as WitPlanV1};
use sys::compose::types::Error as WitError;

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

    fn validate(_plan: WitPlanV1) -> Result<(), WitError> {
        // Full validate() requires a real BlobStore that knows which
        // component digests are present. That lands once the host
        // preopens a blobs/ directory via wasi:filesystem and we wire
        // it through. Until then, return not-implemented honestly
        // rather than rubber-stamping.
        Err(WitError {
            code: sys::compose::types::ErrorCode::NotImplemented,
            message: "validate() requires a host-preopened blob store; \
                      use serialize() + compute-digest() for now"
                .to_string(),
            context: None,
        })
    }

    fn compute_digest(plan: WitPlanV1) -> Result<Vec<u8>, WitError> {
        let core_plan = adapters::wit_plan_to_core(plan);
        compose_core::plan::compute_plan_digest(&core_plan).map_err(adapters::core_err_to_wit)
    }
}

export!(Component);
