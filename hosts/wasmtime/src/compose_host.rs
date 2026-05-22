//! Host implementation of the `compose:host` WIT package.
//!
//! This is the wasmtime-side counterpart to `libs/compose-orchestrator-wasm`.
//! It uses `wasmtime::component::bindgen!` to generate strongly-typed
//! Rust glue from `wit/compose-host/`, then implements the imported
//! interfaces (`runner`, `invoker`, `runtime-info`) against the
//! wasmtime engine the host already owns.
//!
//! The intent is to make the architectural claim of the project
//! end-to-end demonstrable: a wasm orchestrator component can be
//! loaded by this host, satisfy its imports through the
//! `compose:host` surface, and have its exports called — exactly
//! the way a future wasm-orchestrator would dispatch user requests.
//!
//! Today only the smoke path is wired: the orchestrator's
//! `compose:host/smoke.host-name` export is called, which in turn
//! invokes our `runtime-info.get-fingerprint` import. `runner` and
//! `invoker` are stubbed pending the real orchestrator content
//! that will use them.
use anyhow::Result;
use std::path::Path;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Engine, Store};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// Guest-side preopen path the orchestrator expects for its blob
/// store. Mirror of `BLOBS_DIR` in libs/compose-orchestrator-wasm.
/// If you change one, change the other.
pub const ORCHESTRATOR_BLOBS_PREOPEN: &str = "/blobs";

// Wasmtime's Error type doesn't implement std::error::Error so anyhow's
// .context() doesn't apply directly. This helper trait does the same job.
trait WasmtimeContext<T> {
    fn ctx(self, msg: &'static str) -> Result<T>;
}
impl<T> WasmtimeContext<T> for wasmtime::Result<T> {
    fn ctx(self, msg: &'static str) -> Result<T> {
        self.map_err(|e| anyhow::anyhow!("{msg}: {e:?}"))
    }
}

wasmtime::component::bindgen!({
    path: "../../wit/compose-host",
    world: "orchestrator",
});

use compose::host::invoker::Host as InvokerHost;
use compose::host::invoker::HostInstance;
use compose::host::runner::Host as RunnerHost;
use compose::host::runner::{ExecError, ExecResult, Limits};
use compose::host::runtime_info::Host as RuntimeInfoHost;
use compose::host::runtime_info::Fingerprint;

/// Host-side state shared with every guest call.
pub struct HostState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
    runtime_name: String,
    runtime_version: String,
    host_target: String,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

// sys:compose/types is referenced by the plan export's signatures.
// It declares only data records (no functions), so the Host trait is
// empty — but it must be implemented for the orchestrator world to
// link.
impl sys::compose::types::Host for HostState {}

impl RuntimeInfoHost for HostState {
    fn get_fingerprint(&mut self) -> Fingerprint {
        Fingerprint {
            runtime_name: self.runtime_name.clone(),
            runtime_version: self.runtime_version.clone(),
            // Empty placeholder: a real implementation would hash the
            // engine's enabled-features set. The shape is established;
            // the content is the next milestone.
            engine_features_hash: Vec::new(),
            host_target: self.host_target.clone(),
        }
    }

    fn supported_imports(&mut self) -> Vec<String> {
        // Placeholder: real impl walks the linker. Returning the
        // standard WASI set + nothing else for now.
        vec![
            "wasi:cli/run@0.2.0".to_string(),
            "wasi:filesystem/types@0.2.0".to_string(),
            "wasi:clocks/wall-clock@0.2.0".to_string(),
            "wasi:io/streams@0.2.0".to_string(),
        ]
    }
}

impl RunnerHost for HostState {
    fn run_cli(
        &mut self,
        _component_bytes: Vec<u8>,
        _args: Vec<String>,
        _env: Vec<(String, String)>,
        _stdin: Vec<u8>,
        _limits: Limits,
    ) -> Result<ExecResult, ExecError> {
        // Pending the wiring through to the existing exec.rs machinery.
        // The orchestrator smoke POC does not exercise this path.
        Err(ExecError::HostError(
            "runner.run-cli not yet implemented".to_string(),
        ))
    }
}

impl InvokerHost for HostState {
    fn instantiate(
        &mut self,
        _component_bytes: Vec<u8>,
        _limits: Limits,
    ) -> Result<wasmtime::component::Resource<compose::host::invoker::Instance>, ExecError>
    {
        Err(ExecError::HostError(
            "invoker.instantiate not yet implemented".to_string(),
        ))
    }
}

impl HostInstance for HostState {
    fn get_export(
        &mut self,
        _self_: wasmtime::component::Resource<compose::host::invoker::Instance>,
        _name: String,
    ) -> Option<u32> {
        unreachable!("instance handles are never minted by the stub instantiate")
    }

    fn list_exports(
        &mut self,
        _self_: wasmtime::component::Resource<compose::host::invoker::Instance>,
    ) -> Vec<String> {
        unreachable!("instance handles are never minted by the stub instantiate")
    }

    fn call_with_cbor(
        &mut self,
        _self_: wasmtime::component::Resource<compose::host::invoker::Instance>,
        _export_id: u32,
        _args_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, ExecError> {
        unreachable!("instance handles are never minted by the stub instantiate")
    }

    fn drop(
        &mut self,
        _self_: wasmtime::component::Resource<compose::host::invoker::Instance>,
    ) -> wasmtime::Result<()> {
        Ok(())
    }
}

/// One-shot loader: parses the orchestrator component, wires WASI +
/// `compose:host` into the linker, and instantiates.
///
/// If `blobs_preopen` is `Some(host_path)`, the host directory at
/// `host_path` is preopened into the guest at
/// [`ORCHESTRATOR_BLOBS_PREOPEN`] so the orchestrator can run
/// `plan.validate` and any other operation that needs the blob CAS.
/// Callers that only exercise pure functions (smoke, plan.serialize,
/// plan.compute-digest) can pass `None`.
pub fn instantiate_orchestrator(
    engine: &Engine,
    orchestrator_wasm: &[u8],
    blobs_preopen: Option<&Path>,
) -> Result<(Store<HostState>, Orchestrator)> {
    let component = Component::new(engine, orchestrator_wasm)
        .ctx("failed to parse orchestrator component bytes")?;

    let mut linker = Linker::<HostState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .ctx("failed to add WASI to orchestrator linker")?;
    Orchestrator::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |state| state)
        .ctx("failed to add compose:host bindings to orchestrator linker")?;

    let mut wasi_builder = WasiCtxBuilder::new();
    if let Some(path) = blobs_preopen {
        // Ensure the host-side directory exists before we hand
        // wasmtime an ambient handle to it; otherwise the guest's
        // create_dir_all-of-an-existing-dir succeeds vacuously and
        // the test below would silently pass without validating.
        std::fs::create_dir_all(path)
            .map_err(|e| anyhow::anyhow!("failed to create blob preopen directory: {e}"))?;
        wasi_builder
            .preopened_dir(
                path,
                ORCHESTRATOR_BLOBS_PREOPEN,
                DirPerms::all(),
                FilePerms::all(),
            )
            .ctx("failed to preopen blobs directory")?;
    }

    let state = HostState {
        wasi_ctx: wasi_builder.build(),
        wasi_table: ResourceTable::new(),
        runtime_name: "wasmtime".to_string(),
        runtime_version: env!("CARGO_PKG_VERSION").to_string(),
        host_target: std::env::consts::ARCH.to_string()
            + "-"
            + std::env::consts::OS,
    };

    let mut store = Store::new(engine, state);
    let orchestrator = Orchestrator::instantiate(&mut store, &component, &linker)
        .ctx("failed to instantiate orchestrator component")?;
    Ok((store, orchestrator))
}

/// Load an orchestrator component and call its
/// `compose:host/smoke.host-name` export.
///
/// Returns the string the orchestrator reports — which the smoke
/// component constructs by importing `runtime-info.get-fingerprint`
/// and returning the runtime name. A successful round-trip proves
/// the host imports, guest exports, and bindgen wiring are aligned.
pub fn run_smoke(engine: &Engine, orchestrator_wasm: &[u8]) -> Result<String> {
    let (mut store, orchestrator) = instantiate_orchestrator(engine, orchestrator_wasm, None)?;
    orchestrator
        .compose_host_smoke()
        .call_host_name(&mut store)
        .ctx("orchestrator smoke.host-name call failed")
}

/// Load a *composed* orchestrator component (orchestrator wac-plugged
/// with the secure-log component) and call its
/// `compose:host/smoke.audit-demo` export.
///
/// The orchestrator appends `count` audit entries for `tenant` through
/// compose-core's AuditLogger — backed inside the wasm sandbox by the
/// composed secure-log component — then verifies the tenant's hash
/// chain and returns the head sequence number. A successful call
/// proves tamper-evident audit works end-to-end from the wasm
/// orchestrator via component composition.
///
/// Requires the *composed* artifact (compose_orchestrator_composed.wasm).
/// The raw orchestrator alone leaves secure-log:log/log unsatisfied and
/// will fail to instantiate.
pub fn run_audit_demo(
    engine: &Engine,
    composed_orchestrator_wasm: &[u8],
    tenant: &str,
    count: u32,
) -> Result<u64> {
    let (mut store, orchestrator) =
        instantiate_orchestrator(engine, composed_orchestrator_wasm, None)?;
    orchestrator
        .compose_host_smoke()
        .call_audit_demo(&mut store, tenant, count)
        .ctx("orchestrator smoke.audit-demo call failed")?
        .map_err(|e| anyhow::anyhow!("audit-demo returned error: {e}"))
}

/// Load an orchestrator component and call its `compose:host/smoke.digest`
/// export with the given bytes.
///
/// The digest is computed *inside the wasm component* by dispatching
/// to `compose_core::blobs::compute_digest` — the same function the
/// native host uses for blob CAS. A successful round-trip proves the
/// orchestrator's pure-Rust logic is reachable from a wasm guest, not
/// just from native callers.
pub fn run_digest(engine: &Engine, orchestrator_wasm: &[u8], bytes: &[u8]) -> Result<Vec<u8>> {
    let (mut store, orchestrator) = instantiate_orchestrator(engine, orchestrator_wasm, None)?;
    orchestrator
        .compose_host_smoke()
        .call_digest(&mut store, bytes)
        .ctx("orchestrator smoke.digest call failed")
}

/// Construct a minimal `sys:compose/plan.plan-v1` value that's valid
/// enough for serialize / compute-digest to round-trip through. The
/// fields are deliberately ordinary — what's being tested is the WIT
/// boundary, not the plan validator's business rules.
pub fn sample_plan() -> exports::sys::compose::plan::PlanV1 {
    use exports::sys::compose::plan as p;
    use sys::compose::types as t;
    p::PlanV1 {
        version: "1".to_string(),
        root: "root".to_string(),
        components: vec![p::ComponentSpec {
            id: "root".to_string(),
            digest: vec![0u8; 32],
            source: None,
        }],
        bindings: vec![],
        secrets: vec![],
        policy: t::Policy {
            determinism: t::DeterminismMode::Strict,
            capabilities: vec![],
            tenant: None,
            limits: t::ResourceLimits {
                cpu_ms: None,
                memory_bytes: None,
                io_ops: None,
            },
        },
    }
}

/// Load an orchestrator component and call its `sys:compose/plan.compute-digest`
/// export against the given plan. The plan crosses the WIT boundary as
/// a structured record; the orchestrator wasm converts it to a
/// compose-core PlanV1, serializes to canonical CBOR, hashes, and
/// returns the 32-byte digest.
pub fn run_plan_compute_digest(
    engine: &Engine,
    orchestrator_wasm: &[u8],
    plan: exports::sys::compose::plan::PlanV1,
) -> Result<Vec<u8>> {
    let (mut store, orchestrator) = instantiate_orchestrator(engine, orchestrator_wasm, None)?;
    let outcome = orchestrator
        .sys_compose_plan()
        .call_compute_digest(&mut store, &plan)
        .ctx("plan.compute-digest call failed")?;
    outcome.map_err(|e| anyhow::anyhow!("plan.compute-digest returned error: {e:?}"))
}

/// Round-trip a plan through `sys:compose/plan.serialize` and
/// `sys:compose/plan.deserialize`. Returns the deserialized plan,
/// which the caller can compare against the original to check
/// that the WIT boundary preserves every field.
pub fn run_plan_roundtrip(
    engine: &Engine,
    orchestrator_wasm: &[u8],
    plan: exports::sys::compose::plan::PlanV1,
) -> Result<exports::sys::compose::plan::PlanV1> {
    let (mut store, orchestrator) = instantiate_orchestrator(engine, orchestrator_wasm, None)?;
    let plan_iface = orchestrator.sys_compose_plan();

    let bytes = plan_iface
        .call_serialize(&mut store, &plan)
        .ctx("plan.serialize call failed")?
        .map_err(|e| anyhow::anyhow!("plan.serialize returned error: {e:?}"))?;

    let plan_iface = orchestrator.sys_compose_plan();
    let restored = plan_iface
        .call_deserialize(&mut store, &bytes)
        .ctx("plan.deserialize call failed")?
        .map_err(|e| anyhow::anyhow!("plan.deserialize returned error: {e:?}"))?;
    Ok(restored)
}

/// Load an orchestrator component with the host's blob directory
/// preopened at [`ORCHESTRATOR_BLOBS_PREOPEN`], and call
/// `sys:compose/plan.validate`. The orchestrator opens a BlobStore
/// against the preopen and checks that every component digest in
/// the plan is present.
pub fn run_plan_validate(
    engine: &Engine,
    orchestrator_wasm: &[u8],
    host_blobs_dir: &Path,
    plan: exports::sys::compose::plan::PlanV1,
) -> Result<Result<(), exports::sys::compose::plan::Error>> {
    let (mut store, orchestrator) =
        instantiate_orchestrator(engine, orchestrator_wasm, Some(host_blobs_dir))?;
    orchestrator
        .sys_compose_plan()
        .call_validate(&mut store, &plan)
        .ctx("plan.validate call failed")
}

/// Variant of `sample_plan` that takes a real component digest.
/// Used by validate tests that need the plan to reference a blob
/// the host actually has on disk.
pub fn sample_plan_with_digest(component_digest: Vec<u8>) -> exports::sys::compose::plan::PlanV1 {
    use exports::sys::compose::plan as p;
    use sys::compose::types as t;
    p::PlanV1 {
        version: "1".to_string(),
        root: "root".to_string(),
        components: vec![p::ComponentSpec {
            id: "root".to_string(),
            digest: component_digest,
            source: None,
        }],
        bindings: vec![],
        secrets: vec![],
        policy: t::Policy {
            determinism: t::DeterminismMode::Strict,
            capabilities: vec![],
            tenant: None,
            limits: t::ResourceLimits {
                cpu_ms: None,
                memory_bytes: None,
                io_ops: None,
            },
        },
    }
}

/// Convenience for the bindgen-generated `add_to_linker` signature
/// (`F: Fn(&mut T) -> &mut Self::Data`).
struct HasSelf<T>(std::marker::PhantomData<T>);
impl<T: 'static> wasmtime::component::HasData for HasSelf<T> {
    type Data<'a> = &'a mut T;
}
