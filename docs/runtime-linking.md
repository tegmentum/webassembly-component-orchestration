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
convention. As of Phase 6, `invoker` is fully re-implemented on the shared
dynlink instantiation base (`dynlink::instantiate_owned` / `OwnedInstance`), so
there is one runtime-instantiation path — including typed `call-with-cbor`
structured invocation (`crate::cbor_val`). See Phase 6 below.

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

### Payload envelope (recommended convention)

The host forwards `payload` bytes opaquely, so the wire format is an application
concern. To keep providers and consumers evolvable, encode `payload` as a
**CBOR envelope** carrying a schema id and version rather than a bare blob:

```
{
  "schema": "acme:widgets/resize",   // stable identifier for the message type
  "v":      1,                         // envelope/schema version (integer)
  "body":   <CBOR value>               // the actual request/response payload
}
```

Guidelines:

- **Bump `v`** for breaking changes to `body`; a provider can accept multiple
  versions during migration and reject unknown ones with an `error`.
- **Namespace `schema`** (reverse-DNS or a WIT-style `pkg:iface/op`) so a single
  endpoint can multiplex several message types via the `method` argument.
- Reuse `secure_log::CborEncoder` (already a host dependency) so no new
  serialization stack is introduced.

This is a convention, not a host requirement — the runtime never inspects the
bytes. The bundled examples (`dynlink-echo-provider`, `dynlink-endpoint-consumer`)
use a deliberately trivial plain-bytes protocol to keep the mechanism legible;
real providers should adopt the envelope.

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

### Non-goal: cross-instance resource passing

Passing component-model **resources** (handles) between a consumer and a
runtime-linked provider is a deliberate **non-goal**, not a pending feature:

- **The wire format precludes it.** Both runtime-linking flavors move *opaque
  bytes* (the `endpoint` `handle(method, list<u8>) -> list<u8>` contract) or
  CBOR (`invoker.call-with-cbor`). A resource is a live handle into one
  instance's table, not a value — it has no byte/CBOR representation, so
  `crate::cbor_val` rejects `own`/`borrow` (and resource `Val`s) explicitly.
- **Handles are nominal and instance-scoped.** Even with a typed boundary, a
  resource type defined in one instance does not type-match "the same" resource
  in another; `wasm-compose` only unifies them because it merges the components
  into one instance graph. Across separate stores (which is the whole point of
  the per-provider isolation here) there is no shared type identity.
- **What it would take.** A genuinely different *typed linking mode* (the
  approach explicitly rejected in favor of the uniform endpoint), plus a host
  resource-translation table mapping provider-handle ↔ host `Resource<T>` ↔
  consumer-handle per resource type, with ownership/borrow tracking across the
  boundary. That is a large subsystem with no current use case.
- **The supported pattern.** Exchange *values* (bytes/CBOR); if a provider owns
  a stateful resource, keep it inside the provider and address it by an
  application-level id in the message payload (the secret-token indirection
  pattern), rather than handing the raw handle across the boundary.

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

**Wired in later phases** (historical note — these were deferred at Phase 2 and
are now complete): policy gating + audit (Phase 5); `TrustStore` constructed in
`CompositorHost` (Phase 4); and the live guest-driven exec entrypoint that runs
a guest importing `linker` (Phase 7, below).

### Phase 3 — Resolve by id + determinism/exec-key ✅

Implemented in `hosts/wasmtime/src/dynlink.rs` and `hosts/wasmtime/src/exec.rs`:

- **`resolve_by_id`**: `DynState` holds an in-memory `id -> digest` registry
  (`register_id`); `resolve_by_id` looks the id up and delegates to
  `resolve_by_digest` so the trust and determinism gates apply uniformly. An
  unknown id is rejected with `InvalidInput`.
- **Determinism gate**: `DynState` carries the plan's `DeterminismMode`.
  Resolution is refused under `Strict` (`ExecCapabilityDenied`) before any trust
  check or instantiation. `Audit` and `Relaxed` permit it — `Audit` exists to
  allow-and-record non-deterministic operations, and Phase 5 logs each
  resolution under it. (This refines the original "Relaxed only" wording: it is
  really "anything but Strict".)
- **Exec-key**: `compute_exec_key` now takes the set of runtime-resolved
  provider digests and folds them via `fold_resolved_providers` (sorted, domain-
  separated, no-op when empty so static keys are unchanged). `DynState`
  accumulates the resolved set (`resolved_providers()`) during execution —
  necessary because a guest-driven resolution set is only known after the fact;
  flavor A (Phase 4) can pass the plan's bound digests up front instead.

**Tests**: `resolve_by_id_uses_registry`, `strict_determinism_rejects_resolution`
(dynlink); `empty_resolved_set_does_not_change_key`,
`resolved_providers_change_the_key`, `resolved_set_order_is_deterministic`
(exec). The static `run_cli` path passes an empty resolved set, so existing
exec-keys are byte-identical to before.

**Still not wired**: nothing yet *calls* `compute_exec_key` with a non-empty set
or constructs a `DynState` in the live exec path — that integration (plus the
`TrustStore` in `CompositorHost`) lands with Phase 4.

### Phase 4 — Flavor A (late-bound plan imports) ✅

- **`linkage` field**: `PlanV1` gains `linkage: Linkage` (`Static` default,
  `Runtime`), skipped from the canonical encoding when `Static` so existing
  plan digests are byte-identical. It is mirrored into the WIT `plan-v1`
  record as a `linkage-mode` enum (`%static`/`runtime`); the orchestrator-wasm
  adapters map it both directions. Because the digest is computed on the
  *core* plan (where `Static` is omitted), adding the WIT field leaves plan
  digests unchanged (verified by the orchestrator smoke + conformance suites).
- **Cross-store routing**: rather than `func_wrap` + shared state, an
  `endpoint-consumer` world lets the host *import* `endpoint`; `ConsumerState`
  **owns** the bound provider's `Store` and satisfies the import via a trait
  call into it (`hosts/wasmtime/src/dynlink.rs`). `run_cli_with_endpoint`
  instantiates the provider, routes the consumer's import, runs `wasi:cli/run`,
  and captures output — the two components stay in separate stores.
- **Exec integration**: `ExecHandler::run_cli` dispatches on `plan.linkage`;
  `run_cli_runtime_linked` finds the root consumer + the single endpoint
  binding's provider, **trust-gates** the provider digest, folds it into the
  exec-key (activating the Phase 3 machinery), and runs. The static path is
  untouched. `CompositorHost` now constructs a `TrustStore` and threads it into
  `ExecHandler` — closing the long-standing "TrustStore not wired" gap.
- **validate**: rejects `Runtime` linkage under `Strict` determinism, failing
  fast at plan time.
- **Example**: `examples/dynlink-endpoint-consumer/` — a CLI that imports
  `endpoint`, calls `handle`, and prints the reply; wired into `build-all.sh`.

**Tests**: `flavor_a_routes_consumer_endpoint_to_provider` (dynlink, mechanism);
end-to-end `runtime_linked_plan_runs_consumer_with_bound_provider` and
`runtime_linked_plan_rejects_untrusted_provider` (`tests/runtime_linking.rs`,
full stack through `CompositorHost`/`run_cli`). The conformance golden suite
still passes, confirming static-plan digests are unchanged.

**By design**: flavor A binds **one** endpoint provider per plan — a consumer
imports `compose:dynlink/endpoint` exactly once, so a single binding satisfies
it (>1 binding is rejected at exec with a pointer to flavor B). Multiple
providers are served by **flavor B**, where the guest resolves any number of
them on demand via `compose:dynlink/linker`.

**Endpoint-shape validation**: at exec time the host introspects the component
types (`Component::component_type().imports()/exports()`) and rejects a
runtime-linked plan whose provider doesn't **export** `compose:dynlink/endpoint`
or whose consumer doesn't **import** it — a clear `PlanInvalidGraph` error
rather than a cryptic instantiation failure. (This lives host-side because the
portable `compose-core` `validate` has no wasm engine to introspect with.)

### Phase 5 — Policy verbs, audit, and docs ✅

- **Capability verbs**: `dynlink:resolve` / `dynlink:invoke` are added to the
  default `HostPolicy` allow-list (`policy.rs`); a plan must still *declare*
  them to use them. Flavor A gates in `run_cli_runtime_linked` (both verbs
  required, else `ExecCapabilityDenied`). Flavor B gates in `DynState`:
  `resolve_by_digest` requires `dynlink:resolve`, and each `DynInstance`
  snapshots the loader's grant so `invoke` requires `dynlink:invoke` and a
  resolved component can never exceed the loader's grant.
- **Audit**: the flavor-A run records the resolved provider digest in the
  tamper-evident exec log (`success (runtime-linked, provider=<digest>)`).
- **Envelope**: documented above (CBOR `{schema, v, body}`).

**Tests**: `missing_capability_is_rejected` (resolve denied with no grant;
invoke denied when only `resolve` is granted). Existing flavor-A e2e plans now
declare the two capabilities.

**Note**: the guest-driven flavor-B exec path now exists (Phase 7) and audits
the resolved-provider set; both flavors are audited.

### Phase 6 — Reimplement `compose:host/invoker` on the dynlink base ✅

- **Shared base**: `dynlink::instantiate_owned` instantiates *any* component
  in a fresh WASI store and returns an `OwnedInstance` (raw `Instance` + its
  own `Store` + the enumerated callable functions). This is the single
  runtime-instantiation primitive.
- **Function enumeration**: `enumerate_callables` walks top-level exported
  functions and the functions of each top-level exported interface (one level
  deep, named `iface#func`), recording each function's `ComponentExportIndex`
  and component-model parameter/result `Type`s.
- **invoker** (`compose_host.rs`): `instantiate`, `list-exports`, `get-export`,
  and `drop` are real implementations over `OwnedInstance` (handles bridged by
  `Resource::new_own(rep)`); malformed bytes surface as `invalid-component`.
- **`call-with-cbor` (now implemented)**: `crate::cbor_val` performs
  type-directed CBOR↔`Val` marshalling — decode the CBOR argument array against
  the export's parameter types, call the function, re-encode the result `Val`s
  as CBOR. Wire conventions: scalars→CBOR scalars; `list<u8>`→byte string
  (other lists→array); `record`→keyed map; `tuple`→array; `option`→inner-or-null;
  `result`→`{ok|err: v}`; `variant`→`{case: payload}`; `enum`→text; `flags`→array;
  `map`→map. Resources/futures/streams error (not representable).
- **`limits` enforcement**: invoker instances run in a shared sandbox engine
  (`dynlink::sandbox_engine`) built with fuel + epoch interruption.
  `memory_bytes` caps linear memory via `StoreLimits` (denies growth past the
  cap); `cpu_ms` becomes a fuel budget (approximate instruction-per-ms factor),
  exhaustion → `limit-exceeded`; `timeout_ms` becomes an epoch deadline (a
  single background ticker advances the engine epoch every `EPOCH_TICK`),
  exhaustion → `timed-out`. **Not applicable**: `stdio_buffer_bytes` (function
  invocation captures no stdio).

**Tests**: `invoker_lifecycle_runs_on_dynlink_base`,
`invoker_rejects_invalid_component`, `invoker_call_with_cbor_round_trips`
(calls the echo provider's `handle(string, list<u8>) -> result<list<u8>, error>`
end to end), plus six `cbor_val` unit tests for the marshalling.

### Phase 7 — Guest-driven flavor B through `run_cli` ✅

Flavor B is now reachable through the normal execution path, not just the
host API:

- **Dispatch**: `ExecHandler::run_cli` (for `linkage:runtime`) compiles the
  root and checks whether it imports `compose:dynlink/linker`
  (`dynlink::imports_linker`). If so it runs the guest-driven path; otherwise
  it falls back to flavor A's endpoint binding.
- **`dynlink::run_cli_dlopen`**: builds a `DynState` wired to the host's
  engine/blobs/trust, registers the plan's components as the `id -> digest`
  registry, adds WASI + the `linker` import, and runs the guest as a
  `wasi:cli/run` command. The guest resolves providers on demand
  (`resolve-by-id`/`resolve-by-digest`), each trust- and capability-gated. The
  resolved-digest set is recorded in the audit log. Flavor B is **not cached**
  (the resolved set is only known after the run, so a pre-run cache lookup
  isn't possible).
- **Example**: `examples/dynlink-dlopen-guest/` — a CLI that imports `linker`,
  resolves `provider` by id, calls `upper`, and prints the result.

**Test**: `guest_driven_dlopen_runs_through_run_cli` runs a no-binding
`linkage:runtime` plan through `CompositorHost`/`run_cli`; the guest dlopens
the (trusted) echo provider and prints the transformed output.

### Phase 8 — `compose:host/runner` on the shared base ✅

The last `compose:host` stub is implemented: `runner.run-cli` runs a plain
WASI CLI component (`wasi:cli/run`) with args/env/stdin and captured stdio,
under the same `SandboxLimits` as invoker (memory via `StoreLimits`, CPU via
fuel, wall-clock via epoch — and here `stdio_buffer_bytes` *does* apply,
bounding the captured output). Fuel exhaustion → `limit-exceeded`, epoch
deadline → `timed-out`. Backed by `dynlink::run_cli_command`. With this,
**both** `compose:host` capabilities (`runner` + `invoker`) run on the shared
`crate::dynlink` instantiation base — no stubs remain.

**Example**: `examples/hello-component/` (a plain WASI CLI, with a `spin`
arg for limit tests). **Tests**: `runner_runs_plain_cli`,
`runner_enforces_timeout`.

## Resources

- [COMPOSITION_INTEGRATION.md](../COMPOSITION_INTEGRATION.md) — the static model
- `hosts/wasmtime/src/compose_host.rs` — the existing host-bridge pattern
- `wit/sys-compose/trust.wit` — signature/digest verification reused at load time
- `libs/compose-core/src/policy.rs` — policy enforcement extended with dynlink verbs
- [Component Model Spec](https://github.com/WebAssembly/component-model)
