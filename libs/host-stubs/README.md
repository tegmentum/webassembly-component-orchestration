# host-stubs

Static-return stub components that satisfy the compose-orchestrator's
non-WASI, non-secure-log imports at `wac plug` composition time.

The compose-orchestrator wasm declares imports that model the
handoff from the native embedder. In a real deployment those imports
are satisfied by adapter components the embedder ships (a wasmtime
adapter for `tegmentum:runtime`; a startup wrapper for
`host:bootstrap`; a probe for `compose:host/runtime-info`).

For the plugin build path this repo cares about first — a Java host
(Stardog / webassembly4j) that just wants the composed orchestrator
to instantiate and answer `sys:compose/*` calls — none of those
adapters exist yet. Rather than have the Java host mint dozens of
component-model host functions, we plug in trivial static stubs at
build time so the residual import list is WASI-only.

## Stubs

| Crate                   | Exports interface                        | Behaviour                                                                                       |
| ----------------------- | ---------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `runtime-info-stub`     | `compose:host/runtime-info@0.1.0`        | Returns a fixed fingerprint (`stardog` / `1.0.0` / empty features hash / `jvm`).                |
| `bootstrap-stub`        | `host:bootstrap/bootstrap@0.1.0`         | Returns an empty argument list.                                                                 |
| `runtime-control-stub`  | `tegmentum:runtime/control@0.1.0` + `tegmentum:runtime/types@0.1.0` | Provides the `runtime` resource with a no-op constructor. |

## Building

```
cd libs/host-stubs
cargo component build --release --target wasm32-wasip2
```

Artifacts land in `target/wasm32-wasip2/release/`:
- `runtime_info_stub.wasm`
- `bootstrap_stub.wasm`
- `runtime_control_stub.wasm`

The orchestrator's `build.sh` picks them up automatically after the
secure-log-sqlite composition step.

## Replacing with real implementations

Downstream consumers who need real host data — a real fingerprint,
real argv, or a real runtime frontend — replace the corresponding
stub in the `wac plug` chain with a component that exports the same
interface. No orchestrator recompile needed.
