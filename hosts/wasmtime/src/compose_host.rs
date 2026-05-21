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
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

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
struct HostState {
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

/// Load an orchestrator component from raw wasm bytes and call its
/// `compose:host/smoke.host-name` export.
///
/// Returns the string the orchestrator chose to report — which the
/// smoke component constructs by importing `runtime-info.get-fingerprint`
/// and returning the runtime name. A successful round-trip therefore
/// proves the host imports, guest exports, and bindgen wiring are
/// all aligned end-to-end.
pub fn run_smoke(engine: &Engine, orchestrator_wasm: &[u8]) -> Result<String> {
    let component = Component::new(engine, orchestrator_wasm)
        .ctx("failed to parse orchestrator component bytes")?;

    let mut linker = Linker::<HostState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .ctx("failed to add WASI to orchestrator linker")?;
    Orchestrator::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |state| state)
        .ctx("failed to add compose:host bindings to orchestrator linker")?;

    let state = HostState {
        wasi_ctx: WasiCtxBuilder::new().build(),
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
    let name = orchestrator
        .compose_host_smoke()
        .call_host_name(&mut store)
        .ctx("orchestrator smoke.host-name call failed")?;
    Ok(name)
}

/// Convenience for the bindgen-generated `add_to_linker` signature
/// (`F: Fn(&mut T) -> &mut Self::Data`).
struct HasSelf<T>(std::marker::PhantomData<T>);
impl<T: 'static> wasmtime::component::HasData for HasSelf<T> {
    type Data<'a> = &'a mut T;
}
