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
//! Status: Phase 2 — resolve + invoke by digest (flavor B). Policy
//! gating and dedicated audit logging are consolidated in Phase 5;
//! resolve-by-id and the exec-key/determinism wiring are Phase 3.
use compose_core::blobs::BlobStore;
use compose_core::trust::TrustStore;
use compose_core::types::DeterminismMode;
use std::collections::{BTreeSet, HashMap};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

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

/// Host state for a provider instance's own store. Providers may use
/// WASI (std pulls it in even for trivial logic), so the store carries a
/// minimal WASI context.
struct ProviderState {
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
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
        let component = Component::new(&self.engine, &bytes)
            .map_err(|e| error(ErrorCode::EmitLinkError, format!("failed to load provider component: {e:?}")))?;
        let mut store = Store::new(
            &self.engine,
            ProviderState {
                wasi_ctx: WasiCtxBuilder::new().build(),
                wasi_table: ResourceTable::new(),
            },
        );
        let instance = provider::DynlinkProvider::instantiate(&mut store, &component, &self.provider_linker)
            .map_err(|e| error(ErrorCode::ExecTrap, format!("failed to instantiate provider: {e:?}")))?;

        // 4. Mint the opaque handle. The table assigns a rep against our
        // backing type; the guest sees the same rep as a `Resource<Instance>`.
        let backing = self
            .dyn_table
            .push(DynInstance {
                store,
                provider: instance,
                digest: d.clone(),
            })
            .map_err(|e| error(ErrorCode::InternalError, format!("resource table push failed: {e:?}")))?;

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
        let di = self
            .dyn_table
            .get_mut(&as_backing(&self_))
            .map_err(|e| error(ErrorCode::InternalError, format!("unknown dynlink handle: {e:?}")))?;

        // Borrow the provider accessor and its own store as disjoint
        // fields, then forward the opaque message verbatim.
        let endpoint = di.provider.compose_dynlink_endpoint();
        let result = endpoint
            .call_handle(&mut di.store, &method, &payload)
            .map_err(|e| error(ErrorCode::ExecTrap, format!("provider handle trapped: {e:?}")))?;
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

    /// Build a DynState backed by a temp blob store + trust store, with
    /// the provider blob stored and its digest trusted. The returned
    /// `TempDir` must be kept alive for the duration of the test (it owns
    /// the on-disk blob/trust directories).
    fn fixture(
        provider_bytes: &[u8],
        determinism: DeterminismMode,
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
        let state = DynState::new(engine, blobs, trust, determinism).expect("dyn state");
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
        let handle = state.resolve_by_digest(digest.clone()).expect("resolve by digest");

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

        let handle = state.resolve_by_id("echo".to_string()).expect("resolve by id");
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
}
