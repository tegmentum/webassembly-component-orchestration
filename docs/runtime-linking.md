# Runtime / Dynamic Component Linking

## Overview

Today the orchestrator links components **statically**: a `PlanV1` declares
components and `import-binding`s, and `EmitHandler::compose` merges them into a
single, content-addressed artifact at *emit* time using `wasm-compose`
(`hosts/wasmtime/src/exec.rs:103`). Execution loads that sealed artifact and
adds only WASI to the linker — no further component instantiation happens at
runtime. See [COMPOSITION_INTEGRATION.md](../COMPOSITION_INTEGRATION.md) for the
static model.

This document specifies an **additive** runtime-linking capability that lets
components be resolved and called *at exec time* through a thin native host
bridge, in the same style as the existing `compose:host` bridge
(`hosts/wasmtime/src/compose_host.rs`) and the `Signer` / `SecretManager` /
`AuditLogger` host services. The static path is untouched and remains the
default.

### Relationship to `compose:host/invoker`

The codebase already declares a dynamic-instantiation interface,
`compose:host/invoker` (`wit/compose-host/invoker.wit`), currently stubbed
(`hosts/wasmtime/src/compose_host.rs`). It instantiates a component from raw
bytes, hands back an opaque `instance` handle, and calls exports via
`call-with-cbor` — a **typed-CBOR** convention where the host coerces arguments
using the component's type information. It has no content-addressing and no
trust/policy/audit gating, and is scoped as a host→orchestrator capability.

`compose:dynlink` is the **general mechanism**: digest/id resolution, trust +
policy + audit gating, and the uniform opaque-byte `endpoint` calling
convention. The plan is to **re-implement `invoker` on top of `compose:dynlink`**
once the bridge is established (tracked separately), so there is one runtime
instantiation path rather than two. Until then the two coexist; `invoker`
remains the typed-CBOR path and `dynlink` the uniform-byte path.

Two flavors are in scope:

- **Flavor B — guest-driven dynamic linking (`dlopen`-style).** A guest decides
  *at runtime* which component to load (by digest or id) and calls into it
  through an opaque host-owned handle. The plan need not know. **Primary
  deliverable, built first.**
- **Flavor A — late-bound plan imports.** The same `import-binding`s as today,
  but routed at exec time instead of merged at emit time. Cheaper subset, built
  on the same bridge.

### Calling convention: uniform message endpoint

Both flavors use a single **uniform message endpoint** rather than typed
per-interface marshalling. Every dynamically-loadable provider exports one
function:

```wit
handle: func(method: string, payload: list<u8>) -> result<list<u8>, error>
```

The host forwards bytes and never interprets the payload. This is a plugin /
in-process-RPC contract. The payload encoding is an application concern; the
recommended convention is a **CBOR envelope with a schema/version tag**, reusing
the existing `secure_log::CborEncoder` so no new dependency is introduced.

**Why uniform bytes and not typed `Val` marshalling:**

| | Uniform endpoint (chosen) | Typed `Val` marshalling (rejected) |
|---|---|---|
| Runtime introspection | None | Per-call signature lookup + coercion |
| Codegen | None | Generic `Val` marshaller |
| Cross-instance resources | N/A — only bytes cross | Hard: resource handles are nominal, don't type-match across instances |
| Type safety | App-level (schema in the envelope) | Host-side before dispatch |
| Provider requirement | Must export `handle` (plugin shape) | Any WIT interface |

The decisive simplification: **nothing but bytes crosses the boundary, so the
nominal-resource type-identity problem disappears entirely.** The cost is no
host-side type checking — a malformed call surfaces as a deserialization error
*inside the guest*, and providers must be written as plugins. Components needing
arbitrary typed WIT interfaces across the boundary continue to use static
composition.

## Architecture

```
                    guest component (flavor B)
                            │  import compose:dynlink/linker
                            ▼
        ┌───────────────────────────────────────────────┐
        │            compose:dynlink host bridge          │
        │            (bindgen! + impl on HostState)       │
        │                                                 │
        │  resolve-by-digest ─┐                           │
        │  resolve-by-id    ──┤  trust.verify-digest      │
        │                     │  policy.check_dynlink     │
        │                     ▼                           │
        │             ┌──────────────┐   push handle      │
        │             │  BlobStore   │──────────────┐     │
        │             │   (CAS)      │              ▼     │
        │             └──────────────┘      ┌──────────────┐
        │                                   │  dyn_table   │
        │  instance.invoke(method,payload)  │ ResourceTable│
        │     │  policy.check_invoke         │ <DynInstance>│
        │     ▼  audit.log_dynlink           └──────┬───────┘
        │   endpoint.call_handle ───────────────────┘
        │            │  (bytes through, no marshalling)
        └────────────┼────────────────────────────────────┘
                     ▼
            provider component
              export compose:dynlink/endpoint  (handle: method,bytes -> bytes)
```

### New WIT package — `wit/compose-dynlink/`

```wit
package compose:dynlink@0.1.0;

interface endpoint {                        // shape every loadable provider exports
    use compose:types/types.{error};
    handle: func(method: string, payload: list<u8>) -> result<list<u8>, error>;
}

interface linker {
    use compose:types/types.{error, digest};
    resource instance {
        invoke: func(method: string, payload: list<u8>) -> result<list<u8>, error>;
    }
    resolve-by-digest: func(d: digest) -> result<instance, error>;
    resolve-by-id: func(id: string) -> result<instance, error>;
}

world dynlink-guest    { import linker; }     // a guest that dlopens others (flavor B)
world dynlink-provider { export endpoint; }   // a component that wants to be loadable
```

### Host side — `HostState` registry

The bridge follows the existing `compose_host.rs` pattern (`bindgen!` then
`impl ...Host for HostState`). New state on the host:

```rust
struct DynInstance {
    endpoint: Endpoint,          // bindgen-typed handle to the provider's `handle` export
    digest: Digest,
    capabilities: EnforcedPolicy,// what this instance is permitted to do
}

// added to HostState:
//   dyn_table: ResourceTable        hands out Resource<DynInstance>
//   blobs:     BlobStore            clone of the CAS
//   trust:     TrustVerifier        wit/sys-compose/trust.wit
//   policy:    PolicyEnforcer
//   audit:     AuditLogger
```

```rust
fn resolve_by_digest(&mut self, d: Digest) -> Result<Resource<DynInstance>, Error> {
    self.policy.check_dynlink_allowed(&d)?;         // capability gate
    self.trust.verify_digest(&d)?;                  // reuse trust.wit — no unsigned code
    let bytes = self.blobs.get(&d)?;
    let component = Component::new(&engine, &bytes)?;
    let endpoint = Endpoint::instantiate(&mut store, &component, &self.dynlink_linker)?;
    self.audit.log_dynlink(&d, "resolve", "ok")?;   // reuse AuditLogger
    Ok(self.dyn_table.push(DynInstance { endpoint, digest: d, capabilities })?)
}

fn invoke(&mut self, h: Resource<DynInstance>, method: String, payload: Vec<u8>)
    -> Result<Vec<u8>, Error>
{
    let di = self.dyn_table.get(&h)?;
    self.policy.check_invoke(&di.capabilities, &method)?;
    let out = di.endpoint.call_handle(&mut self.store, &method, &payload)?;  // post_return via bindgen
    self.audit.log_dynlink(&di.digest, &method, "ok")?;
    out
}
```

### Where it plugs into exec

The seam is the linker setup in `hosts/wasmtime/src/exec.rs`, beside
`wasmtime_wasi::p2::add_to_linker_sync` (`exec.rs:225`, `exec.rs:371`). The new
bridge's `add_to_linker` is registered only when runtime linking is requested;
the static `emit.compose` path (`exec.rs:103`) is otherwise unchanged.

## Flavor A — late-bound plan imports

With the uniform endpoint, flavor A's consumer *imports* `compose:dynlink/endpoint`
and the host routes the plan's `import-binding` to the bound provider's
`endpoint` export. It is plan-declared plugin wiring, not transparent typed
trampolining.

```rust
// linkage:runtime — route the consumer's endpoint import to the bound provider,
// for each ImportBinding (libs/compose-core/src/types.rs:184)
let provider = providers[&binding.provider_id].clone();   // pre-instantiated Endpoint
linker.instance("compose:dynlink/endpoint")?.func_wrap(
    "handle",
    move |mut store, (method, payload): (String, Vec<u8>)| {
        provider.call_handle(&mut store, &method, &payload)   // bytes through, no encoding
    },
)?;
```

A new `linkage` field is added to `PlanV1` (`libs/compose-core/src/types.rs:201`)
and `plan-v1` (`wit/sys-compose/plan.wit`), defaulting to `static`. `validate`
(`plan.wit`) rejects a `linkage:runtime` binding whose endpoints don't both
speak `endpoint`.

## Trust, determinism, and policy

These constraints apply to both flavors and are non-negotiable for the dynamic
path:

- **Trust at load time.** Static composition verifies inputs once at emit.
  Dynamic `resolve_*` must run `trust.verify-digest`
  (`wit/sys-compose/trust.wit:28`) on every loaded blob *before*
  `Component::new`, or it becomes an unsigned-code-execution path.
- **Determinism.** Runtime linking breaks the sealed-artifact reproducibility
  the `exec-key` depends on (`exec.rs:453`). Resolved provider digests must be
  folded into the exec-key, and dynamic linking is permitted only under
  `DeterminismMode::Relaxed` (`libs/compose-core/src/types.rs:146`). The
  resolved digest set is recorded in the audit event.
- **Policy.** Two new capability verbs — `dynlink:resolve` and `dynlink:invoke` —
  extend `PolicyEnforcer` (`libs/compose-core/src/policy.rs:187`). Each
  `DynInstance` carries a per-instance capability set so a dynamically loaded
  component cannot exceed the loader's grant.

## Scope and limitations (v1)

- **Value-typed messaging only.** Only bytes cross the boundary. Resource
  handles never transit, by construction — this is why the bridge is small.
- **Providers must be plugins.** A component must export `endpoint` to be
  loadable dynamically. Arbitrary typed WIT interfaces across the boundary
  remain the domain of static composition.
- **No host-side type checking.** The schema contract lives in the CBOR envelope
  and the guests' agreement on it.

## Comparison with static composition

| Aspect | Static composition | Runtime linking (this doc) |
|--------|--------------------|----------------------------|
| **When** | Emit time | Exec time |
| **Output** | Single sealed artifact | Root + on-demand providers |
| **Binding decided by** | Plan, at build | Guest at runtime (B) / plan at exec (A) |
| **Boundary contract** | Native WIT interfaces | `handle(method, bytes) -> bytes` |
| **Resources across boundary** | Yes (unified by composer) | No (bytes only) |
| **Trust check** | Once, at emit | Per load, at resolve |
| **Determinism** | Reproducible artifact | Relaxed mode only; digests in exec-key |
| **Use case** | Sealed deployable | Plugins, late binding, host-mediated calls |

## Implementation plan

Phases are ordered so each is independently demonstrable. Flavor B is usable at
the end of Phase 2.

### Phase 1 — Bridge skeleton

- Add `wit/compose-dynlink/` with `endpoint`, `linker`, and the two worlds.
- `bindgen!` the bridge in a new `hosts/wasmtime/src/dynlink.rs`, mirroring
  `compose_host.rs`.
- Extend `HostState` with `dyn_table: ResourceTable` and a `DynInstance` type;
  add the bridge's `add_to_linker` call next to the WASI registration in
  `exec.rs`.
- *Exit:* host compiles and instantiates a guest that imports `linker` (no
  behavior yet).

### Phase 2 — Resolve + invoke by digest (flavor B usable) ✅

Implemented in `hosts/wasmtime/src/dynlink.rs`:

- `resolve_by_digest`: `trust.verify_digest` → `blobs.get` → `Component::new`
  → instantiate the provider in its **own** `Store<ProviderState>` → push the
  `DynInstance` into `dyn_table`. Each provider owns its store, so the imported
  `linker` methods never need the calling guest's store.
- `instance.invoke`: look up the `DynInstance`, forward `(method, payload)`
  verbatim to the provider's `endpoint.call_handle`, lower any provider error.
- `instance` resource handles: the generated `Instance` marker is bridged to the
  backing `DynInstance` via `Resource::new_own(rep)` (whole-interface `with:`
  remapping is the only granularity wasmtime 44 supports, so resource-level
  remap is done by rep instead). `drop` releases the provider's store.
- Example provider: `examples/dynlink-echo-provider/` (exports
  `compose:dynlink/endpoint`, methods `echo`/`upper`/`len`), built by its
  `build.sh` and wired into `examples/build-all.sh`.

**Calling convention note.** The example uses a trivial bytes protocol, not the
CBOR envelope — the envelope (Phase 5) is the *recommended* application
convention, not a host requirement, and the point here is the host mechanism.

**Tests** (`dynlink::tests`): `resolve_and_invoke_echo_provider` round-trips
`echo`/`upper`/`len` against the real wasm provider; `untrusted_digest_is_rejected`
confirms the trust gate; `linker_registration_type_checks` covers registration.
The integration test drives the host trait methods directly (standing in for a
guest that imports `linker`) and skips gracefully if the provider isn't built.

**Deferred to later phases (not yet wired):**

- **Policy gating** (`dynlink:resolve` / `dynlink:invoke`) and **dedicated audit
  logging** — consolidated in Phase 5. Phase 2 gates on trust only.
- **`TrustStore` in `CompositorHost`** — still not constructed in
  `hosts/wasmtime/src/lib.rs`; the test builds its own. A live exec entrypoint
  that instantiates a guest importing `linker` (and therefore needs the host to
  own a `TrustStore` + build a `DynState`) arrives with Phase 4's exec
  integration. Until then runtime linking is exercised at the host-API level.

### Phase 3 — Resolve by id + determinism/exec-key

- Add an id→digest registry (extend the CAS or the trusted set in `trust.wit`);
  implement `resolve_by_id`.
- Fold resolved provider digests into `compute_exec_key` (`exec.rs:453`).
- Gate dynamic linking on `DeterminismMode::Relaxed`; reject otherwise with a
  clear error.
- *Exit:* exec-key changes when a resolved provider changes; non-Relaxed plans
  are rejected.

### Phase 4 — Flavor A (late-bound plan imports)

- Add `linkage: static | runtime` to `PlanV1` (`types.rs`) and `plan-v1`
  (`plan.wit`), default `static`.
- Implement the `func_wrap` routing of `import-binding`s to provider `endpoint`
  exports in `exec.rs`, active only under `linkage:runtime`.
- Extend `validate` (`plan.wit` impl) to reject runtime bindings whose endpoints
  don't both speak `endpoint`.
- *Exit:* a `linkage:runtime` plan executes with imports routed at exec time;
  the static path is unchanged for `linkage:static`.

### Phase 5 — Policy verbs, audit, and docs

- Add `dynlink:resolve` / `dynlink:invoke` capability verbs to `PolicyEnforcer`
  (`policy.rs`) and per-instance capability sets on `DynInstance`.
- Ensure every resolve/invoke is audited with the resolved digest set.
- Document the CBOR payload envelope (schema/version tag) as the
  inter-component contract.
- *Exit:* a component cannot resolve/invoke without the capability; audit log
  shows the full dynamic-linking trail.

## Resources

- [COMPOSITION_INTEGRATION.md](../COMPOSITION_INTEGRATION.md) — the static model
- `hosts/wasmtime/src/compose_host.rs` — the existing host-bridge pattern
- `wit/sys-compose/trust.wit` — signature/digest verification reused at load time
- `libs/compose-core/src/policy.rs` — policy enforcement extended with dynlink verbs
- [Component Model Spec](https://github.com/WebAssembly/component-model)
