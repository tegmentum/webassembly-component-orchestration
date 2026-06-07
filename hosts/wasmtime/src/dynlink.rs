//! Host implementation of the `compose:dynlink` WIT package — runtime /
//! dynamic component linking ("dlopen for components").
//!
//! A guest imports `compose:dynlink/linker` to resolve and instantiate
//! another component at exec time and call into it through an opaque,
//! host-owned handle. The host verifies trust before instantiation and
//! forwards opaque byte messages to the provider's
//! `compose:dynlink/endpoint.handle` export — no typed marshalling
//! crosses the boundary.
//!
//! Each resolved provider runs in its **own** [`Store`], owned by the
//! [`DynInstance`] behind the handle (exactly as `compose:host/invoker`
//! documents). This is why the imported `linker` methods only need
//! `&mut DynState` and never the guest's store: instantiating and
//! calling a provider touches only the provider's own store.
//!
//! This module also provides the shared instantiation primitives used by
//! the `compose:host` capabilities: `instantiate_owned` / `OwnedInstance`
//! (invoker), `run_cli_with_endpoint` (flavor A), `run_cli_dlopen`
//! (flavor B), and `run_cli_command` (runner) — all running in isolated,
//! limit-enforced stores on the shared fuel + epoch `sandbox_engine`.
use compose_core::blobs::BlobStore;
use compose_core::trust::TrustStore;
use compose_core::types::DeterminismMode;
use std::collections::{BTreeSet, HashMap};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Engine, Store};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// Capability a plan must be granted to resolve (instantiate) a component
/// at runtime.
pub const CAP_RESOLVE: &str = "dynlink:resolve";
/// Capability a plan must be granted to invoke a resolved instance.
pub const CAP_INVOKE: &str = "dynlink:invoke";

/// The uniform endpoint interface name (version-agnostic prefix). Used to
/// validate that runtime-linked components actually speak `endpoint`.
pub const ENDPOINT_INTERFACE: &str = "compose:dynlink/endpoint";

/// The runtime-linking `linker` interface name (version-agnostic prefix).
/// A root component importing this drives flavor B (guest-driven dlopen).
pub const LINKER_INTERFACE: &str = "compose:dynlink/linker";

/// Whether a compiled component imports the `linker` interface (flavor B).
pub fn imports_linker(engine: &Engine, component: &Component) -> bool {
    component
        .component_type()
        .imports(engine)
        .any(|(name, _)| name.starts_with(LINKER_INTERFACE))
}

/// Whether a compiled component exports the `endpoint` interface.
fn exports_endpoint(engine: &Engine, component: &Component) -> bool {
    component
        .component_type()
        .exports(engine)
        .any(|(name, _)| name.starts_with(ENDPOINT_INTERFACE))
}

/// Whether a compiled component imports the `endpoint` interface.
fn imports_endpoint(engine: &Engine, component: &Component) -> bool {
    component
        .component_type()
        .imports(engine)
        .any(|(name, _)| name.starts_with(ENDPOINT_INTERFACE))
}

/// Typed bindings for instantiating and calling a *provider* component —
/// one that exports `compose:dynlink/endpoint`. Kept in its own module
/// so its generated `compose::dynlink` / `sys::compose` types don't
/// collide with the guest-side bindings below.
mod provider {
    wasmtime::component::bindgen!({
        path: "../../wit/compose-dynlink",
        world: "dynlink-provider",
    });
}

/// Typed bindings for a *consumer* world (flavor A): the host imports
/// `endpoint` and satisfies it by forwarding to a bound provider. Kept
/// in its own module for the same reason as `provider`.
mod consumer {
    wasmtime::component::bindgen!({
        path: "../../wit/compose-dynlink",
        world: "endpoint-consumer",
    });
}

/// Host state for an isolated instance's own store. Components may use
/// WASI (std pulls it in even for trivial logic), so the store carries a
/// minimal WASI context.
pub struct ProviderState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
    /// Memory/instance resource limits for this store (unlimited by
    /// default; set for `invoker` instances per their `limits`).
    limits: wasmtime::StoreLimits,
}

impl WasiView for ProviderState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

/// A resolved provider, owned by the host and handed to the guest as an
/// opaque `instance` handle. Holds its own store so calls into the
/// provider never touch the calling guest's store.
pub struct DynInstance {
    store: Store<ProviderState>,
    provider: provider::DynlinkProvider,
    /// Content digest the provider was resolved from (used by Phase 3's
    /// exec-key wiring and audit).
    #[allow(dead_code)]
    digest: Vec<u8>,
    /// Capabilities granted to this instance, snapshotted from the loader
    /// at resolve time so a resolved component can never exceed the
    /// loader's grant.
    capabilities: BTreeSet<String>,
}

wasmtime::component::bindgen!({
    path: "../../wit/compose-dynlink",
    world: "dynlink-guest",
});

use compose::dynlink::linker::{Host as LinkerHost, HostInstance, Instance};
use sys::compose::types::{Error, ErrorCode};
use wasmtime::component::Resource;

/// Retype a guest-facing `Resource<Instance>` to the host backing type
/// `Resource<DynInstance>`. The two share the same table rep — the type
/// parameter is only a host-side compile-time tag — so this is a sound
/// reinterpretation, not a cast across distinct table entries.
fn as_backing(r: &Resource<Instance>) -> Resource<DynInstance> {
    Resource::new_own(r.rep())
}

/// Host-side state shared with every guest call into the dynlink bridge.
pub struct DynState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
    /// Owns the live [`DynInstance`] values handed out as `instance`
    /// handles. Distinct from `wasi_table` so dropping a dynlink handle
    /// never disturbs WASI-owned resources.
    dyn_table: ResourceTable,
    /// Engine used to compile and instantiate resolved providers.
    engine: Engine,
    /// Content-addressed store providers are loaded from.
    blobs: BlobStore,
    /// Trust gate: a digest must be trusted before it is instantiated.
    trust: TrustStore,
    /// Pre-built linker (WASI only) used to instantiate every provider.
    provider_linker: Linker<ProviderState>,
    /// Determinism mode of the executing plan. Runtime linking is a
    /// non-deterministic operation, so it is refused under `Strict`.
    determinism: DeterminismMode,
    /// Capabilities the executing plan was granted. `resolve`/`invoke` are
    /// gated on the relevant verb being present.
    granted: BTreeSet<String>,
    /// Registry mapping a stable component id to its content digest, used
    /// by `resolve-by-id`. Populated by the host before execution.
    registry: HashMap<String, Vec<u8>>,
    /// Digests resolved during this execution, in sorted order. Exposed
    /// so the exec path can fold them into the exec-key and audit record
    /// (a guest-driven resolution set is only known after the fact).
    resolved: BTreeSet<Vec<u8>>,
}

impl DynState {
    /// Construct a dynlink host state wired to the host's blob store,
    /// trust store, and engine. WASI is registered on the provider
    /// linker so resolved providers can link the WASI surface std needs.
    pub fn new(
        engine: Engine,
        blobs: BlobStore,
        trust: TrustStore,
        determinism: DeterminismMode,
        granted: BTreeSet<String>,
    ) -> anyhow::Result<Self> {
        let mut provider_linker = Linker::<ProviderState>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut provider_linker)
            .map_err(|e| anyhow::anyhow!("failed to add WASI to provider linker: {e:?}"))?;
        Ok(Self {
            wasi_ctx: WasiCtxBuilder::new().build(),
            wasi_table: ResourceTable::new(),
            dyn_table: ResourceTable::new(),
            engine,
            blobs,
            trust,
            provider_linker,
            determinism,
            granted,
            registry: HashMap::new(),
            resolved: BTreeSet::new(),
        })
    }

    /// Register an `id -> digest` mapping for `resolve-by-id`.
    pub fn register_id(&mut self, id: impl Into<String>, digest: Vec<u8>) {
        self.registry.insert(id.into(), digest);
    }

    /// The set of provider digests resolved during this execution, sorted.
    /// The exec path folds this into the exec-key and audit record.
    pub fn resolved_providers(&self) -> &BTreeSet<Vec<u8>> {
        &self.resolved
    }
}

impl WasiView for DynState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

/// Build a host `Error` with the given code and message.
fn error(code: ErrorCode, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
        context: None,
    }
}

/// Lower a provider-side error (distinct Rust type, identical shape)
/// into the guest-facing `Error`.
fn lower_provider_error(e: provider::sys::compose::types::Error) -> Error {
    Error {
        code: ErrorCode::ExecTrap,
        message: format!("provider endpoint error: {}", e.message),
        context: e.context,
    }
}

// `sys:compose/types` is referenced by the linker interface's signatures.
// It declares only data records (no functions), so the Host trait is
// empty — but it must be implemented for the world to link.
impl sys::compose::types::Host for DynState {}

impl LinkerHost for DynState {
    fn resolve_by_digest(&mut self, d: Vec<u8>) -> Result<Resource<Instance>, Error> {
        // 0. Determinism gate: runtime linking is non-deterministic, so
        // refuse it under Strict. Audit/Relaxed permit it (Phase 5 records
        // each resolution in the audit log under Audit).
        if self.determinism == DeterminismMode::Strict {
            return Err(error(
                ErrorCode::ExecCapabilityDenied,
                "runtime linking is not permitted under strict determinism",
            ));
        }

        // 0b. Capability gate: the plan must hold `dynlink:resolve`.
        if !self.granted.contains(CAP_RESOLVE) {
            return Err(error(
                ErrorCode::ExecCapabilityDenied,
                format!("resolution requires the '{CAP_RESOLVE}' capability"),
            ));
        }

        // 1. Trust gate: refuse to instantiate code that isn't trusted.
        self.trust
            .verify_digest(&d)
            .map_err(|e| error(ErrorCode::TrustUntrustedSource, e.to_string()))?;

        // 2. Load the provider bytes from the content-addressed store.
        let bytes = self
            .blobs
            .get(&d)
            .map_err(|e| error(ErrorCode::BlobNotFound, e.to_string()))?;

        // 3. Compile and instantiate the provider in its own store.
        let component = Component::new(&self.engine, &bytes).map_err(|e| {
            error(
                ErrorCode::EmitLinkError,
                format!("failed to load provider component: {e:?}"),
            )
        })?;
        let mut store = Store::new(
            &self.engine,
            ProviderState {
                wasi_ctx: WasiCtxBuilder::new().build(),
                wasi_table: ResourceTable::new(),
                limits: wasmtime::StoreLimits::default(),
            },
        );
        let instance =
            provider::DynlinkProvider::instantiate(&mut store, &component, &self.provider_linker)
                .map_err(|e| {
                error(
                    ErrorCode::ExecTrap,
                    format!("failed to instantiate provider: {e:?}"),
                )
            })?;

        // 4. Mint the opaque handle. The table assigns a rep against our
        // backing type; the guest sees the same rep as a `Resource<Instance>`.
        let backing = self
            .dyn_table
            .push(DynInstance {
                store,
                provider: instance,
                digest: d.clone(),
                // Snapshot the loader's grant; a resolved component cannot
                // exceed it.
                capabilities: self.granted.clone(),
            })
            .map_err(|e| {
                error(
                    ErrorCode::InternalError,
                    format!("resource table push failed: {e:?}"),
                )
            })?;

        // 5. Record the resolved digest for the exec-key / audit record.
        self.resolved.insert(d);
        Ok(Resource::new_own(backing.rep()))
    }

    fn resolve_by_id(&mut self, id: String) -> Result<Resource<Instance>, Error> {
        let digest = self.registry.get(&id).cloned().ok_or_else(|| {
            error(
                ErrorCode::InvalidInput,
                format!("unknown component id: {id}"),
            )
        })?;
        // Delegate so the determinism and trust gates apply uniformly.
        self.resolve_by_digest(digest)
    }
}

impl HostInstance for DynState {
    fn invoke(
        &mut self,
        self_: Resource<Instance>,
        method: String,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Error> {
        let di = self.dyn_table.get_mut(&as_backing(&self_)).map_err(|e| {
            error(
                ErrorCode::InternalError,
                format!("unknown dynlink handle: {e:?}"),
            )
        })?;

        // Capability gate: this instance must hold `dynlink:invoke`.
        if !di.capabilities.contains(CAP_INVOKE) {
            return Err(error(
                ErrorCode::ExecCapabilityDenied,
                format!("invoke requires the '{CAP_INVOKE}' capability"),
            ));
        }

        // Borrow the provider accessor and its own store as disjoint
        // fields, then forward the opaque message verbatim.
        let endpoint = di.provider.compose_dynlink_endpoint();
        let result = endpoint
            .call_handle(&mut di.store, &method, &payload)
            .map_err(|e| {
                error(
                    ErrorCode::ExecTrap,
                    format!("provider handle trapped: {e:?}"),
                )
            })?;
        result.map_err(lower_provider_error)
    }

    fn drop(&mut self, rep: Resource<Instance>) -> wasmtime::Result<()> {
        // Releasing the handle drops the provider's store and any linear
        // memory it held.
        let _ = self.dyn_table.delete(as_backing(&rep))?;
        Ok(())
    }
}

/// Convenience for the bindgen-generated `add_to_linker` signature
/// (`F: Fn(&mut T) -> &mut Self::Data`). Mirrors the helper in
/// [`crate::compose_host`].
struct HasSelf<T>(std::marker::PhantomData<T>);
impl<T: 'static> wasmtime::component::HasData for HasSelf<T> {
    type Data<'a> = &'a mut T;
}

/// Add the `compose:dynlink/linker` import to a guest linker.
///
/// WASI must be added separately (`wasmtime_wasi::p2::add_to_linker_sync`)
/// by the caller, exactly as for the `compose:host` bridge.
pub fn add_to_linker(linker: &mut Linker<DynState>) -> anyhow::Result<()> {
    DynlinkGuest::add_to_linker::<_, HasSelf<DynState>>(linker, |state| state)
        .map_err(|e| anyhow::anyhow!("failed to add compose:dynlink bindings to linker: {e:?}"))
}

/// Result of running a consumer command with a runtime-linked endpoint.
#[derive(Debug)]
pub struct RuntimeLinkOutput {
    pub exit_code: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Execution state for a consumer command (flavor A). It **owns** the
/// bound provider's store, so satisfying the consumer's `endpoint` import
/// is a plain trait call into that store — no shared-state gymnastics,
/// and the two components stay in separate stores.
struct ConsumerState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
    provider_store: Store<ProviderState>,
    provider: provider::DynlinkProvider,
}

impl WasiView for ConsumerState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

impl consumer::sys::compose::types::Host for ConsumerState {}

impl consumer::compose::dynlink::endpoint::Host for ConsumerState {
    fn handle(
        &mut self,
        method: String,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, consumer::sys::compose::types::Error> {
        // Disjoint borrows of `provider` and `provider_store` from &mut self.
        let endpoint = self.provider.compose_dynlink_endpoint();
        let result = endpoint
            .call_handle(&mut self.provider_store, &method, &payload)
            .map_err(|e| {
                consumer_error(
                    ErrorCode::ExecTrap,
                    format!("provider handle trapped: {e:?}"),
                )
            })?;
        result.map_err(|e| {
            consumer_error(
                ErrorCode::ExecTrap,
                format!("provider endpoint error: {}", e.message),
            )
        })
    }
}

/// Build a consumer-side `Error` (a distinct Rust type from the guest-side
/// `Error`, same shape).
fn consumer_error(code: ErrorCode, message: String) -> consumer::sys::compose::types::Error {
    consumer::sys::compose::types::Error {
        // `ErrorCode` is shared structurally across the bindgen modules but
        // is a distinct Rust type per module; remap by value.
        code: match code {
            ErrorCode::ExecTrap => consumer::sys::compose::types::ErrorCode::ExecTrap,
            _ => consumer::sys::compose::types::ErrorCode::InternalError,
        },
        message,
        context: None,
    }
}

/// Build a portable `compose_core` error for the public flavor-A helpers
/// (which return `compose_core::Error`, unlike the bindgen trait impls
/// that must return the generated `Error`).
fn core_error(code: compose_core::types::ErrorCode, message: String) -> compose_core::types::Error {
    compose_core::types::Error::new(code, message)
}

/// Epoch ticker granularity. The wall-clock timeout resolution is one
/// tick; deadlines are rounded up to whole ticks.
pub const EPOCH_TICK: std::time::Duration = std::time::Duration::from_millis(10);

/// Resource limits applied to a sandboxed instance.
#[derive(Default, Clone, Copy)]
pub struct SandboxLimits {
    /// Max linear-memory bytes the guest may allocate (denies growth past it).
    pub memory_bytes: Option<u64>,
    /// CPU budget expressed as wasmtime fuel units. `None` = unbounded.
    /// Requires the engine to have been built with `consume_fuel`.
    pub fuel: Option<u64>,
    /// Wall-clock deadline expressed in [`EPOCH_TICK`] units. `None` =
    /// unbounded. Requires the engine to have `epoch_interruption`.
    pub epoch_ticks: Option<u64>,
}

/// The shared engine for sandboxed `invoker` instances: component model +
/// fuel consumption + epoch interruption (so CPU and wall-clock budgets
/// can be enforced). Built once; a single background thread advances the
/// engine's epoch every [`EPOCH_TICK`] for wall-clock timeouts.
///
/// Cheap to clone; kept alive by any `Store` created from it (and, being a
/// process-lifetime singleton, by the ticker thread).
pub fn sandbox_engine() -> Engine {
    static ENGINE: std::sync::OnceLock<Engine> = std::sync::OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let mut config = wasmtime::Config::new();
            config.wasm_component_model(true);
            config.consume_fuel(true);
            config.epoch_interruption(true);
            // This config is static and valid, so construction cannot fail.
            let engine = Engine::new(&config).expect("sandbox engine config is valid");
            let weak = engine.weak();
            // Daemon ticker: advances the epoch until the engine is dropped
            // (never, for this singleton).
            let _ = std::thread::Builder::new()
                .name("dynlink-epoch-ticker".to_string())
                .spawn(move || loop {
                    std::thread::sleep(EPOCH_TICK);
                    match weak.upgrade() {
                        Some(e) => e.increment_epoch(),
                        None => break,
                    }
                });
            engine
        })
        .clone()
}

/// A component instantiated in its own isolated WASI store, owned by the
/// host. This is the shared base for both the dynlink endpoint path and
/// the `compose:host/invoker` capability, so there is one runtime-
/// instantiation path rather than two.
pub struct OwnedInstance {
    /// The instance's own store. Calls into the instance borrow it `&mut`.
    pub store: Store<ProviderState>,
    /// The instantiated component, with imports satisfied by WASI only.
    pub instance: wasmtime::component::Instance,
    /// Every callable exported function, including those nested one level
    /// inside an exported interface (named `iface#func`). This is what the
    /// `invoker` capability enumerates and calls.
    pub funcs: Vec<CallableFunc>,
}

/// A single callable exported function plus the metadata needed to invoke
/// it: its export index and component-model parameter/result types.
pub struct CallableFunc {
    pub name: String,
    pub index: wasmtime::component::ComponentExportIndex,
    pub params: Vec<wasmtime::component::Type>,
    pub results: Vec<wasmtime::component::Type>,
}

/// Enumerate callable functions: top-level exported functions plus the
/// functions of each top-level exported interface (one level deep).
fn enumerate_callables(
    store: &mut Store<ProviderState>,
    instance: &wasmtime::component::Instance,
    top_names: &[String],
    engine: &Engine,
) -> Vec<CallableFunc> {
    use wasmtime::component::types::ComponentItem;
    let mut funcs = Vec::new();
    for top in top_names {
        let Some((item, idx)) = instance.get_export(&mut *store, None, top) else {
            continue;
        };
        match item {
            ComponentItem::ComponentFunc(cf) => funcs.push(CallableFunc {
                name: top.clone(),
                index: idx,
                params: cf.params().map(|(_, t)| t).collect(),
                results: cf.results().collect(),
            }),
            ComponentItem::ComponentInstance(ci) => {
                let subs: Vec<String> = ci.exports(engine).map(|(n, _)| n.to_string()).collect();
                for sub in subs {
                    if let Some((ComponentItem::ComponentFunc(cf), fidx)) =
                        instance.get_export(&mut *store, Some(&idx), &sub)
                    {
                        funcs.push(CallableFunc {
                            name: format!("{top}#{sub}"),
                            index: fidx,
                            params: cf.params().map(|(_, t)| t).collect(),
                            results: cf.results().collect(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    funcs
}

/// Instantiate any component in a fresh WASI-only store and return the raw
/// instance plus its top-level export names. Errors are returned as a
/// portable `compose_core::Error`.
pub fn instantiate_owned(
    engine: &Engine,
    bytes: &[u8],
    limits: SandboxLimits,
) -> Result<OwnedInstance, compose_core::types::Error> {
    use compose_core::types::ErrorCode as CoreErrorCode;
    let component = Component::new(engine, bytes).map_err(|e| {
        core_error(
            CoreErrorCode::InvalidInput,
            format!("invalid component: {e:?}"),
        )
    })?;
    let exports: Vec<String> = component
        .component_type()
        .exports(engine)
        .map(|(name, _)| name.to_string())
        .collect();
    let mut linker = Linker::<ProviderState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| {
        core_error(
            CoreErrorCode::InternalError,
            format!("failed to add WASI: {e:?}"),
        )
    })?;

    // Memory cap: StoreLimits denies growth beyond the limit (the guest
    // sees memory.grow fail; no host trap).
    let mut limits_builder = wasmtime::StoreLimitsBuilder::new();
    if let Some(max) = limits.memory_bytes {
        limits_builder = limits_builder.memory_size(max as usize);
    }
    let mut store = Store::new(
        engine,
        ProviderState {
            wasi_ctx: WasiCtxBuilder::new().build(),
            wasi_table: ResourceTable::new(),
            limits: limits_builder.build(),
        },
    );
    store.limiter(|s| &mut s.limits);
    // CPU cap via fuel. Best-effort: a no-op error when the engine wasn't
    // built with `consume_fuel`. With fuel enabled the store starts at 0,
    // so we must always set it (u64::MAX ~ unbounded when no limit).
    let _ = store.set_fuel(limits.fuel.unwrap_or(u64::MAX));
    // Wall-clock cap via epoch interruption. With epoch_interruption the
    // store starts at deadline 0, so set it unconditionally. The deadline
    // is `current_epoch + delta`, so the "unbounded" delta is u64::MAX/2
    // (a full u64::MAX overflows once the ticker has advanced the epoch).
    // The background ticker advances the engine epoch; on deadline the
    // guest traps with Trap::Interrupt.
    store.set_epoch_deadline(limits.epoch_ticks.unwrap_or(u64::MAX / 2));
    store.epoch_deadline_trap();

    let instance = linker.instantiate(&mut store, &component).map_err(|e| {
        core_error(
            CoreErrorCode::ExecTrap,
            format!("failed to instantiate: {e:?}"),
        )
    })?;
    let funcs = enumerate_callables(&mut store, &instance, &exports, engine);
    Ok(OwnedInstance {
        store,
        instance,
        funcs,
    })
}

/// Instantiate a provider component (exports `endpoint`) in its own store.
fn instantiate_provider(
    engine: &Engine,
    bytes: &[u8],
) -> Result<(Store<ProviderState>, provider::DynlinkProvider), compose_core::types::Error> {
    use compose_core::types::ErrorCode as CoreErrorCode;
    let component = Component::new(engine, bytes).map_err(|e| {
        core_error(
            CoreErrorCode::EmitLinkError,
            format!("failed to load provider component: {e:?}"),
        )
    })?;
    // Validate the shape up front so a misconfigured binding fails with a
    // clear message instead of a cryptic instantiation error.
    if !exports_endpoint(engine, &component) {
        return Err(core_error(
            CoreErrorCode::PlanInvalidGraph,
            format!("provider does not export {ENDPOINT_INTERFACE}"),
        ));
    }
    let mut linker = Linker::<ProviderState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| {
        core_error(
            CoreErrorCode::InternalError,
            format!("failed to add WASI to provider linker: {e:?}"),
        )
    })?;
    let mut store = Store::new(
        engine,
        ProviderState {
            wasi_ctx: WasiCtxBuilder::new().build(),
            wasi_table: ResourceTable::new(),
            limits: wasmtime::StoreLimits::default(),
        },
    );
    let instance = provider::DynlinkProvider::instantiate(&mut store, &component, &linker)
        .map_err(|e| {
            core_error(
                CoreErrorCode::ExecTrap,
                format!("failed to instantiate provider: {e:?}"),
            )
        })?;
    Ok((store, instance))
}

/// Run a consumer CLI command (`wasi:cli/run`) whose single `endpoint`
/// import is satisfied by `provider_bytes` (flavor A). The provider is
/// instantiated in its own store; the consumer's import is routed to it
/// through [`ConsumerState`]. Trust verification of `provider_bytes` is
/// the caller's responsibility.
pub fn run_cli_with_endpoint(
    engine: &Engine,
    consumer_bytes: &[u8],
    provider_bytes: &[u8],
    args: &[String],
    env: &[(String, String)],
) -> Result<RuntimeLinkOutput, compose_core::types::Error> {
    use compose_core::types::ErrorCode as CoreErrorCode;
    let (provider_store, provider) = instantiate_provider(engine, provider_bytes)?;

    let consumer_component = Component::new(engine, consumer_bytes).map_err(|e| {
        core_error(
            CoreErrorCode::EmitLinkError,
            format!("failed to load consumer component: {e:?}"),
        )
    })?;
    if !imports_endpoint(engine, &consumer_component) {
        return Err(core_error(
            CoreErrorCode::PlanInvalidGraph,
            format!("consumer does not import {ENDPOINT_INTERFACE}; the runtime binding has nothing to satisfy"),
        ));
    }

    let mut linker = Linker::<ConsumerState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| {
        core_error(
            CoreErrorCode::InternalError,
            format!("failed to add WASI to consumer linker: {e:?}"),
        )
    })?;
    consumer::EndpointConsumer::add_to_linker::<_, HasSelf<ConsumerState>>(&mut linker, |s| s)
        .map_err(|e| {
            core_error(
                CoreErrorCode::EmitLinkError,
                format!("failed to add endpoint import to consumer linker: {e:?}"),
            )
        })?;

    let stdout = MemoryOutputPipe::new(64 * 1024);
    let stderr = MemoryOutputPipe::new(64 * 1024);
    let mut builder = WasiCtxBuilder::new();
    builder.args(args);
    for (k, v) in env {
        builder.env(k, v);
    }
    builder
        .stdin(MemoryInputPipe::new(Vec::new()))
        .stdout(stdout.clone())
        .stderr(stderr.clone());

    let state = ConsumerState {
        wasi_ctx: builder.build(),
        wasi_table: ResourceTable::new(),
        provider_store,
        provider,
    };
    let mut store = Store::new(engine, state);

    let command = Command::instantiate(&mut store, &consumer_component, &linker).map_err(|e| {
        core_error(
            CoreErrorCode::ExecTrap,
            format!("failed to instantiate consumer command: {e:?}"),
        )
    })?;
    let exit_code = match command.wasi_cli_run().call_run(&mut store).map_err(|e| {
        core_error(
            CoreErrorCode::ExecTrap,
            format!("consumer run trapped: {e:?}"),
        )
    })? {
        Ok(()) => 0u32,
        Err(()) => 1u32,
    };

    drop(store);
    Ok(RuntimeLinkOutput {
        exit_code,
        stdout: stdout.contents().to_vec(),
        stderr: stderr.contents().to_vec(),
    })
}

/// Run a CLI guest that drives runtime linking itself (flavor B): the
/// guest imports `compose:dynlink/linker` and resolves providers on demand
/// by id/digest. `registry` maps component ids to digests (from the plan);
/// the host's `blobs`/`trust` back resolution, gated by `granted` caps and
/// `determinism`. Returns the run output plus the set of provider digests
/// the guest actually resolved (for the audit record).
#[allow(clippy::too_many_arguments)]
pub fn run_cli_dlopen(
    engine: &Engine,
    guest_bytes: &[u8],
    registry: &[(String, Vec<u8>)],
    blobs: BlobStore,
    trust: TrustStore,
    determinism: DeterminismMode,
    granted: BTreeSet<String>,
    args: &[String],
    env: &[(String, String)],
) -> Result<(RuntimeLinkOutput, BTreeSet<Vec<u8>>), compose_core::types::Error> {
    use compose_core::types::ErrorCode as CoreErrorCode;

    let component = Component::new(engine, guest_bytes).map_err(|e| {
        core_error(
            CoreErrorCode::EmitLinkError,
            format!("failed to load guest component: {e:?}"),
        )
    })?;
    if !imports_linker(engine, &component) {
        return Err(core_error(
            CoreErrorCode::PlanInvalidGraph,
            format!("guest does not import {LINKER_INTERFACE}"),
        ));
    }

    let mut linker = Linker::<DynState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| {
        core_error(
            CoreErrorCode::InternalError,
            format!("failed to add WASI to guest linker: {e:?}"),
        )
    })?;
    add_to_linker(&mut linker)
        .map_err(|e| core_error(CoreErrorCode::EmitLinkError, format!("{e:?}")))?;

    let mut state = DynState::new(engine.clone(), blobs, trust, determinism, granted)
        .map_err(|e| core_error(CoreErrorCode::InternalError, format!("{e:?}")))?;
    for (id, digest) in registry {
        state.register_id(id.clone(), digest.clone());
    }

    // Configure the guest's WASI context (args/env + captured stdio),
    // replacing the default one DynState::new builds.
    let stdout = MemoryOutputPipe::new(64 * 1024);
    let stderr = MemoryOutputPipe::new(64 * 1024);
    let mut builder = WasiCtxBuilder::new();
    builder.args(args);
    for (k, v) in env {
        builder.env(k, v);
    }
    builder
        .stdin(MemoryInputPipe::new(Vec::new()))
        .stdout(stdout.clone())
        .stderr(stderr.clone());
    state.wasi_ctx = builder.build();

    let mut store = Store::new(engine, state);
    let command = Command::instantiate(&mut store, &component, &linker).map_err(|e| {
        core_error(
            CoreErrorCode::ExecTrap,
            format!("failed to instantiate guest command: {e:?}"),
        )
    })?;
    let exit_code = match command
        .wasi_cli_run()
        .call_run(&mut store)
        .map_err(|e| core_error(CoreErrorCode::ExecTrap, format!("guest run trapped: {e:?}")))?
    {
        Ok(()) => 0u32,
        Err(()) => 1u32,
    };

    let resolved = store.data().resolved_providers().clone();
    drop(store);
    Ok((
        RuntimeLinkOutput {
            exit_code,
            stdout: stdout.contents().to_vec(),
            stderr: stderr.contents().to_vec(),
        },
        resolved,
    ))
}

/// Run a plain WASI CLI component (`wasi:cli/run`) with resource limits and
/// captured stdio. This backs the `compose:host/runner` capability: the
/// component imports only WASI (no `compose:dynlink` surface). Uses the
/// shared fuel + epoch sandbox engine so `limits` are enforced; fuel
/// exhaustion → `ExecResourceExhausted`, epoch deadline → `ExecTimeout`.
/// `stdio_cap` bounds the captured stdout/stderr buffers.
pub fn run_cli_command(
    bytes: &[u8],
    args: &[String],
    env: &[(String, String)],
    stdin: &[u8],
    limits: SandboxLimits,
    stdio_cap: usize,
) -> Result<RuntimeLinkOutput, compose_core::types::Error> {
    use compose_core::types::ErrorCode as CoreErrorCode;
    let engine = sandbox_engine();
    let component = Component::new(&engine, bytes).map_err(|e| {
        core_error(
            CoreErrorCode::InvalidInput,
            format!("invalid component: {e:?}"),
        )
    })?;

    let mut linker = Linker::<ProviderState>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| {
        core_error(
            CoreErrorCode::InternalError,
            format!("failed to add WASI: {e:?}"),
        )
    })?;

    let mut limits_builder = wasmtime::StoreLimitsBuilder::new();
    if let Some(max) = limits.memory_bytes {
        limits_builder = limits_builder.memory_size(max as usize);
    }
    let stdout = MemoryOutputPipe::new(stdio_cap);
    let stderr = MemoryOutputPipe::new(stdio_cap);
    let mut builder = WasiCtxBuilder::new();
    builder.args(args);
    for (k, v) in env {
        builder.env(k, v);
    }
    builder
        .stdin(MemoryInputPipe::new(stdin.to_vec()))
        .stdout(stdout.clone())
        .stderr(stderr.clone());

    let mut store = Store::new(
        &engine,
        ProviderState {
            wasi_ctx: builder.build(),
            wasi_table: ResourceTable::new(),
            limits: limits_builder.build(),
        },
    );
    store.limiter(|s| &mut s.limits);
    let _ = store.set_fuel(limits.fuel.unwrap_or(u64::MAX));
    store.set_epoch_deadline(limits.epoch_ticks.unwrap_or(u64::MAX / 2));
    store.epoch_deadline_trap();

    let command = Command::instantiate(&mut store, &component, &linker).map_err(|e| {
        core_error(
            CoreErrorCode::ExecTrap,
            format!("failed to instantiate command: {e:?}"),
        )
    })?;
    let exit_code = match command.wasi_cli_run().call_run(&mut store) {
        Ok(Ok(())) => 0u32,
        Ok(Err(())) => 1u32,
        Err(e) => {
            let code = match e.downcast_ref::<wasmtime::Trap>() {
                Some(wasmtime::Trap::OutOfFuel) => CoreErrorCode::ExecResourceExhausted,
                Some(wasmtime::Trap::Interrupt) => CoreErrorCode::ExecTimeout,
                _ => CoreErrorCode::ExecTrap,
            };
            return Err(core_error(code, format!("command run failed: {e:?}")));
        }
    };

    drop(store);
    Ok(RuntimeLinkOutput {
        exit_code,
        stdout: stdout.contents().to_vec(),
        stderr: stderr.contents().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use compose_core::types::VerificationMetadata;
    use std::path::PathBuf;

    /// Locate the prebuilt echo-provider component. Built by
    /// `examples/dynlink-echo-provider/build.sh`; tests skip gracefully
    /// when it is absent (mirrors `orchestrator_smoke.rs`).
    fn echo_provider_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/dynlink-echo-provider/target/wasm32-wasip2/release/dynlink_echo_provider.wasm")
    }

    /// The consumer is a bin crate, so its artifact keeps the dashed
    /// package name (unlike the cdylib provider's underscored name).
    fn endpoint_consumer_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/dynlink-endpoint-consumer/target/wasm32-wasip2/release/dynlink-endpoint-consumer.wasm")
    }

    /// Build a DynState backed by a temp blob store + trust store, with
    /// the provider blob stored and its digest trusted. The returned
    /// `TempDir` must be kept alive for the duration of the test (it owns
    /// the on-disk blob/trust directories).
    /// Capability set granting both dynlink verbs.
    fn full_grant() -> BTreeSet<String> {
        [CAP_RESOLVE.to_string(), CAP_INVOKE.to_string()]
            .into_iter()
            .collect()
    }

    fn fixture(
        provider_bytes: &[u8],
        determinism: DeterminismMode,
    ) -> (DynState, Vec<u8>, tempfile::TempDir) {
        fixture_with_grant(provider_bytes, determinism, full_grant())
    }

    fn fixture_with_grant(
        provider_bytes: &[u8],
        determinism: DeterminismMode,
        granted: BTreeSet<String>,
    ) -> (DynState, Vec<u8>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("temp dir");
        let blobs = BlobStore::new(tmp.path().join("blobs"), 64 * 1024 * 1024).expect("blob store");
        let digest = blobs.put(provider_bytes).expect("store provider");

        let clock = compose_core::SystemClock::shared();
        let trust = TrustStore::new(tmp.path().join("trust"), clock).expect("trust store");
        trust
            .trust_digest(
                &digest,
                VerificationMetadata {
                    signer: "test".to_string(),
                    timestamp: None,
                    backend: "dev".to_string(),
                },
            )
            .expect("trust the provider digest");

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).expect("engine");
        let state = DynState::new(engine, blobs, trust, determinism, granted).expect("dyn state");
        (state, digest, tmp)
    }

    /// Phase 1 carry-over: the bridge registers against a real engine.
    #[test]
    fn linker_registration_type_checks() {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).unwrap();
        let mut linker = Linker::<DynState>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).expect("WASI registers");
        add_to_linker(&mut linker).expect("compose:dynlink registers");
    }

    /// Phase 2 exit: resolve a real provider by digest and round-trip
    /// messages through its endpoint. Exercises the full host path
    /// (trust -> blobs -> instantiate -> invoke) against a real wasm
    /// component, standing in for a guest that imports `linker`.
    #[test]
    fn resolve_and_invoke_echo_provider() {
        let Ok(bytes) = std::fs::read(echo_provider_path()) else {
            eprintln!(
                "skipping: echo provider not built; run \
                 examples/dynlink-echo-provider/build.sh"
            );
            return;
        };

        let (mut state, digest, _tmp) = fixture(&bytes, DeterminismMode::Relaxed);
        let handle = state
            .resolve_by_digest(digest.clone())
            .expect("resolve by digest");

        // The resolved digest is recorded for exec-key / audit folding.
        assert!(state.resolved_providers().contains(&digest));

        let echoed = state
            .invoke(
                wasmtime::component::Resource::new_own(handle.rep()),
                "echo".to_string(),
                b"hello".to_vec(),
            )
            .expect("echo");
        assert_eq!(echoed, b"hello");

        let upper = state
            .invoke(
                wasmtime::component::Resource::new_own(handle.rep()),
                "upper".to_string(),
                b"hello".to_vec(),
            )
            .expect("upper");
        assert_eq!(upper, b"HELLO");

        let len = state
            .invoke(
                wasmtime::component::Resource::new_own(handle.rep()),
                "len".to_string(),
                b"hello".to_vec(),
            )
            .expect("len");
        assert_eq!(len, b"5");

        state.drop(handle).expect("drop handle");
    }

    /// Untrusted digests must be refused before any instantiation.
    #[test]
    fn untrusted_digest_is_rejected() {
        let Ok(bytes) = std::fs::read(echo_provider_path()) else {
            return;
        };
        let (mut state, _digest, _tmp) = fixture(&bytes, DeterminismMode::Relaxed);
        // A digest that was never trusted.
        let bogus = vec![0u8; 32];
        let err = state.resolve_by_digest(bogus).expect_err("must reject");
        assert!(matches!(err.code, ErrorCode::TrustUntrustedSource));
    }

    /// resolve-by-id maps a registered id to its digest, then applies the
    /// same trust + determinism gates as resolve-by-digest.
    #[test]
    fn resolve_by_id_uses_registry() {
        let Ok(bytes) = std::fs::read(echo_provider_path()) else {
            return;
        };
        let (mut state, digest, _tmp) = fixture(&bytes, DeterminismMode::Relaxed);
        state.register_id("echo", digest.clone());

        let handle = state
            .resolve_by_id("echo".to_string())
            .expect("resolve by id");
        let echoed = state
            .invoke(
                Resource::new_own(handle.rep()),
                "echo".to_string(),
                b"hi".to_vec(),
            )
            .expect("echo");
        assert_eq!(echoed, b"hi");
        assert!(state.resolved_providers().contains(&digest));

        // An unregistered id is rejected.
        let err = state
            .resolve_by_id("nope".to_string())
            .expect_err("unknown id must fail");
        assert!(matches!(err.code, ErrorCode::InvalidInput));
    }

    /// Flavor A: a real consumer command's `endpoint` import is routed at
    /// exec time to a separately-instantiated provider, cross-store. The
    /// consumer sends `upper("hello from consumer")`; the echo provider
    /// uppercases it; the consumer prints the result.
    #[test]
    fn flavor_a_routes_consumer_endpoint_to_provider() {
        let (Ok(provider), Ok(consumer)) = (
            std::fs::read(echo_provider_path()),
            std::fs::read(endpoint_consumer_path()),
        ) else {
            eprintln!(
                "skipping: build examples/dynlink-echo-provider and \
                 examples/dynlink-endpoint-consumer"
            );
            return;
        };

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).unwrap();

        let out = run_cli_with_endpoint(&engine, &consumer, &provider, &[], &[]).expect("run");
        assert_eq!(
            out.exit_code,
            0,
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "HELLO FROM CONSUMER"
        );
    }

    /// Endpoint-shape validation: a provider that doesn't export
    /// `endpoint`, and a consumer that doesn't import it, are both rejected
    /// with a clear error rather than a cryptic instantiation failure.
    /// (The consumer wasm exports no `endpoint`; the provider wasm imports
    /// none — so swapping them exercises each check.)
    #[test]
    fn endpoint_shape_is_validated() {
        let (Ok(provider), Ok(consumer)) = (
            std::fs::read(echo_provider_path()),
            std::fs::read(endpoint_consumer_path()),
        ) else {
            return;
        };
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).unwrap();

        // Provider slot given a component that doesn't export endpoint.
        let err = run_cli_with_endpoint(&engine, &consumer, &consumer, &[], &[])
            .expect_err("provider must export endpoint");
        assert!(
            err.message.contains("does not export"),
            "got: {}",
            err.message
        );

        // Consumer slot given a component that doesn't import endpoint.
        let err = run_cli_with_endpoint(&engine, &provider, &provider, &[], &[])
            .expect_err("consumer must import endpoint");
        assert!(
            err.message.contains("does not import"),
            "got: {}",
            err.message
        );
    }

    /// Runtime linking is refused under strict determinism, before any
    /// trust check or instantiation.
    #[test]
    fn strict_determinism_rejects_resolution() {
        let Ok(bytes) = std::fs::read(echo_provider_path()) else {
            return;
        };
        let (mut state, digest, _tmp) = fixture(&bytes, DeterminismMode::Strict);
        let err = state
            .resolve_by_digest(digest)
            .expect_err("strict must reject");
        assert!(matches!(err.code, ErrorCode::ExecCapabilityDenied));
        assert!(state.resolved_providers().is_empty());
    }

    /// Resolution requires the `dynlink:resolve` capability; invocation
    /// requires `dynlink:invoke`.
    #[test]
    fn missing_capability_is_rejected() {
        let Ok(bytes) = std::fs::read(echo_provider_path()) else {
            return;
        };

        // No capabilities granted: resolve is refused.
        let (mut bare, digest, _t1) =
            fixture_with_grant(&bytes, DeterminismMode::Relaxed, BTreeSet::new());
        let err = bare
            .resolve_by_digest(digest.clone())
            .expect_err("must deny resolve");
        assert!(matches!(err.code, ErrorCode::ExecCapabilityDenied));

        // resolve granted but not invoke: resolve succeeds, invoke is refused.
        let resolve_only: BTreeSet<String> = [CAP_RESOLVE.to_string()].into_iter().collect();
        let (mut state, digest, _t2) =
            fixture_with_grant(&bytes, DeterminismMode::Relaxed, resolve_only);
        let handle = state.resolve_by_digest(digest).expect("resolve allowed");
        let err = state
            .invoke(
                Resource::new_own(handle.rep()),
                "echo".to_string(),
                b"x".to_vec(),
            )
            .expect_err("must deny invoke");
        assert!(matches!(err.code, ErrorCode::ExecCapabilityDenied));
    }
}
