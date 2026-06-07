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
use compose::host::runtime_info::Fingerprint;
use compose::host::runtime_info::Host as RuntimeInfoHost;

/// Host-side state shared with every guest call.
pub struct HostState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
    runtime_name: String,
    runtime_version: String,
    host_target: String,
    /// Backing store for live `invoker` instances, handed out as opaque
    /// `instance` resource handles. Reuses the dynlink instantiation base.
    /// (Each instance carries its own fuel-enabled sandbox engine.)
    invoker_table: ResourceTable,
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

/// Retype an `invoker` instance handle to the dynlink `OwnedInstance`
/// backing it (same table rep; see the equivalent in `dynlink`).
fn invoker_backing(
    r: &wasmtime::component::Resource<compose::host::invoker::Instance>,
) -> wasmtime::component::Resource<crate::dynlink::OwnedInstance> {
    wasmtime::component::Resource::new_own(r.rep())
}

impl InvokerHost for HostState {
    fn instantiate(
        &mut self,
        component_bytes: Vec<u8>,
        limits: Limits,
    ) -> Result<wasmtime::component::Resource<compose::host::invoker::Instance>, ExecError> {
        // Sandbox the instance in its own fuel-enabled engine so memory and
        // CPU caps can be enforced. `memory_bytes` caps linear memory
        // (StoreLimits); `cpu_ms` becomes a fuel budget via an approximate
        // instruction-per-ms factor. `timeout_ms` (true wall-clock) needs
        // epoch interruption + a ticker and is not yet enforced;
        // `stdio_buffer_bytes` does not apply to function invocation.
        const FUEL_PER_MS: u64 = 1_000_000;
        let engine =
            crate::dynlink::sandbox_engine().map_err(|e| ExecError::HostError(e.message))?;
        let sandbox_limits = crate::dynlink::SandboxLimits {
            memory_bytes: limits.memory_bytes,
            fuel: limits.cpu_ms.map(|ms| ms.saturating_mul(FUEL_PER_MS)),
        };
        let owned = crate::dynlink::instantiate_owned(&engine, &component_bytes, sandbox_limits)
            .map_err(|e| ExecError::InvalidComponent(e.message))?;
        let backing = self
            .invoker_table
            .push(owned)
            .map_err(|e| ExecError::HostError(format!("resource table push failed: {e:?}")))?;
        Ok(wasmtime::component::Resource::new_own(backing.rep()))
    }
}

impl HostInstance for HostState {
    fn get_export(
        &mut self,
        self_: wasmtime::component::Resource<compose::host::invoker::Instance>,
        name: String,
    ) -> Option<u32> {
        let owned = self.invoker_table.get(&invoker_backing(&self_)).ok()?;
        owned
            .funcs
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }

    fn list_exports(
        &mut self,
        self_: wasmtime::component::Resource<compose::host::invoker::Instance>,
    ) -> Vec<String> {
        self.invoker_table
            .get(&invoker_backing(&self_))
            .map(|owned| owned.funcs.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default()
    }

    fn call_with_cbor(
        &mut self,
        self_: wasmtime::component::Resource<compose::host::invoker::Instance>,
        export_id: u32,
        args_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, ExecError> {
        // Structured invocation: decode the CBOR argument array against the
        // export's parameter types, call it, and re-encode the results as
        // CBOR. See crate::cbor_val for the wire conventions.
        let owned = self
            .invoker_table
            .get_mut(&invoker_backing(&self_))
            .map_err(|e| ExecError::HostError(format!("unknown instance handle: {e:?}")))?;

        let func_meta = owned
            .funcs
            .get(export_id as usize)
            .ok_or_else(|| ExecError::HostError(format!("no export with id {export_id}")))?;
        let index = func_meta.index;
        let params = func_meta.params.clone();
        let result_count = func_meta.results.len();

        let args = crate::cbor_val::decode_params(&args_cbor, &params)
            .map_err(|e| ExecError::HostError(format!("decoding args: {e}")))?;

        let func = owned
            .instance
            .get_func(&mut owned.store, index)
            .ok_or_else(|| ExecError::HostError("export is not a function".to_string()))?;

        let mut results = vec![wasmtime::component::Val::Bool(false); result_count];
        func.call(&mut owned.store, &args, &mut results)
            .map_err(|e| {
                // A fuel/CPU-budget exhaustion surfaces as an OutOfFuel trap.
                if matches!(
                    e.downcast_ref::<wasmtime::Trap>(),
                    Some(wasmtime::Trap::OutOfFuel)
                ) {
                    ExecError::LimitExceeded("cpu/fuel budget exceeded".to_string())
                } else {
                    ExecError::Trap(format!("{e:?}"))
                }
            })?;
        // wasmtime 45: post_return is a no-op and deprecated, so it is not
        // called here.

        crate::cbor_val::encode_results(&results)
            .map_err(|e| ExecError::HostError(format!("encoding results: {e}")))
    }

    fn drop(
        &mut self,
        self_: wasmtime::component::Resource<compose::host::invoker::Instance>,
    ) -> wasmtime::Result<()> {
        // Releasing the handle drops the instance's store and linear memory.
        let _ = self.invoker_table.delete(invoker_backing(&self_))?;
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
        host_target: std::env::consts::ARCH.to_string() + "-" + std::env::consts::OS,
        invoker_table: ResourceTable::new(),
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

#[cfg(test)]
mod invoker_tests {
    use super::*;
    use std::path::PathBuf;

    fn echo_provider() -> Option<Vec<u8>> {
        std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../examples/dynlink-echo-provider/target/wasm32-wasip2/release/dynlink_echo_provider.wasm",
        ))
        .ok()
    }

    fn host_state() -> HostState {
        HostState {
            wasi_ctx: WasiCtxBuilder::new().build(),
            wasi_table: ResourceTable::new(),
            runtime_name: "test".to_string(),
            runtime_version: "0".to_string(),
            host_target: "test".to_string(),
            invoker_table: ResourceTable::new(),
        }
    }

    fn no_limits() -> Limits {
        Limits {
            cpu_ms: None,
            memory_bytes: None,
            timeout_ms: None,
            stdio_buffer_bytes: None,
        }
    }

    fn handle_to(
        h: &wasmtime::component::Resource<compose::host::invoker::Instance>,
    ) -> wasmtime::component::Resource<compose::host::invoker::Instance> {
        wasmtime::component::Resource::new_own(h.rep())
    }

    /// invoker.instantiate / list-exports / get-export / drop now run on
    /// the shared dynlink instantiation base (no longer stubbed).
    #[test]
    fn invoker_lifecycle_runs_on_dynlink_base() {
        let Some(bytes) = echo_provider() else {
            eprintln!("skipping: build examples/dynlink-echo-provider");
            return;
        };
        let mut state = host_state();

        let handle = InvokerHost::instantiate(&mut state, bytes, no_limits()).expect("instantiate");

        // The echo provider exports `compose:dynlink/endpoint`'s `handle`;
        // it surfaces as a callable `iface#func` entry.
        let exports = HostInstance::list_exports(&mut state, handle_to(&handle));
        assert!(
            exports.iter().any(|e| e.ends_with("#handle")),
            "expected a #handle export, got {exports:?}"
        );

        let known = exports[0].clone();
        assert!(HostInstance::get_export(&mut state, handle_to(&handle), known).is_some());
        assert!(
            HostInstance::get_export(&mut state, handle_to(&handle), "nope".to_string()).is_none()
        );

        HostInstance::drop(&mut state, handle).expect("drop");
    }

    /// call-with-cbor: structured invocation of the echo provider's
    /// `handle(method, payload) -> result<list<u8>, error>` through the
    /// CBOR marshalling layer.
    #[test]
    fn invoker_call_with_cbor_round_trips() {
        use ciborium::value::Value as Cbor;

        let Some(bytes) = echo_provider() else {
            return;
        };
        let mut state = host_state();
        let handle = InvokerHost::instantiate(&mut state, bytes, no_limits()).expect("instantiate");

        let exports = HostInstance::list_exports(&mut state, handle_to(&handle));
        let handle_fn = exports
            .iter()
            .find(|n| n.ends_with("#handle"))
            .expect("handle export")
            .clone();
        let id = HostInstance::get_export(&mut state, handle_to(&handle), handle_fn).unwrap();

        // args: ("upper", b"hi") — string + list<u8> (as a CBOR byte string).
        let args = Cbor::Array(vec![
            Cbor::Text("upper".into()),
            Cbor::Bytes(b"hi".to_vec()),
        ]);
        let mut args_cbor = Vec::new();
        ciborium::into_writer(&args, &mut args_cbor).unwrap();

        let out = HostInstance::call_with_cbor(&mut state, handle_to(&handle), id, args_cbor)
            .expect("call-with-cbor");

        // result: [ {"ok": b"HI"} ]  (result<list<u8>, error> = Ok(...))
        let decoded: Cbor = ciborium::from_reader(&out[..]).expect("result is CBOR");
        let Cbor::Array(items) = decoded else {
            panic!("expected array, got {decoded:?}")
        };
        assert_eq!(items.len(), 1, "one result value");
        let Cbor::Map(entries) = &items[0] else {
            panic!("expected result map")
        };
        let (k, v) = &entries[0];
        assert!(matches!(k, Cbor::Text(s) if s == "ok"), "expected ok arm");
        match v {
            Cbor::Bytes(b) => assert_eq!(b, b"HI"),
            other => panic!("expected uppercased bytes, got {other:?}"),
        }

        HostInstance::drop(&mut state, handle).expect("drop");
    }

    /// Explicit memory + CPU limits are applied to the instance (a generous
    /// budget still allows a normal call to succeed). This exercises the
    /// StoreLimits + fuel plumbing without breaking valid execution.
    #[test]
    fn call_succeeds_under_generous_limits() {
        use ciborium::value::Value as Cbor;

        let Some(bytes) = echo_provider() else {
            return;
        };
        let mut state = host_state();
        let limits = Limits {
            cpu_ms: Some(60_000),
            memory_bytes: Some(256 * 1024 * 1024),
            timeout_ms: None,
            stdio_buffer_bytes: None,
        };
        let handle =
            InvokerHost::instantiate(&mut state, bytes, limits).expect("instantiate under limits");

        let exports = HostInstance::list_exports(&mut state, handle_to(&handle));
        let f = exports
            .iter()
            .find(|n| n.ends_with("#handle"))
            .expect("handle export")
            .clone();
        let id = HostInstance::get_export(&mut state, handle_to(&handle), f).unwrap();

        let args = Cbor::Array(vec![Cbor::Text("echo".into()), Cbor::Bytes(b"ok".to_vec())]);
        let mut args_cbor = Vec::new();
        ciborium::into_writer(&args, &mut args_cbor).unwrap();

        let out = HostInstance::call_with_cbor(&mut state, handle_to(&handle), id, args_cbor)
            .expect("call under limits");
        let decoded: Cbor = ciborium::from_reader(&out[..]).unwrap();
        let Cbor::Array(items) = decoded else {
            panic!("array")
        };
        let Cbor::Map(entries) = &items[0] else {
            panic!("map")
        };
        assert!(matches!(&entries[0].0, Cbor::Text(s) if s == "ok"));

        HostInstance::drop(&mut state, handle).expect("drop");
    }

    /// Malformed component bytes surface as invalid-component, not a stub.
    #[test]
    fn invoker_rejects_invalid_component() {
        let mut state = host_state();

        let err = InvokerHost::instantiate(&mut state, vec![0, 1, 2, 3], no_limits())
            .expect_err("garbage must fail");
        assert!(matches!(err, ExecError::InvalidComponent(_)));
    }
}
