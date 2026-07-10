# `compose-orchestrator-wasm`

Wasm component that consumes the [`compose:host`](../../wit/compose-host)
WIT package and exports the first `sys:compose` interface. Proves the
"orchestrator as a wasm component" story end-to-end against
[`hosts/wasmtime`](../../hosts/wasmtime): host imports flow through the
`compose:host` bindgen surface, and guest exports are callable via the
same wasmtime component the host owns.

This crate is **not** part of the workspace (it's listed in the root
`Cargo.toml`'s `exclude`, targets `wasm32-wasip2`). Build it on its own:

```bash
./build.sh
```

That produces two artifacts under `target/wasm32-wasip2/release/`:

- `compose_orchestrator_wasm.wasm` — raw component. Imports
  `secure-log:log/log@0.1.0` and cannot be instantiated standalone.
- `compose_orchestrator_composed.wasm` — the raw component `wac plug`'d
  with a `secure-log-sqlite` component so the secure-log import is
  satisfied. The wasmtime host loads this artifact. Override the
  secure-log component location with `SECURE_LOG_WASM`.

If `wasm-tools` is on `$PATH`, `build.sh` also prints the composed
component's declared world.

## Exports

The `orchestrator` world (declared in `wit/world.wit`) currently
exports:

- **`compose:host/smoke`** — three probes used by the wasmtime host's
  integration tests:
  - `host-name()` — calls the imported
    `compose:host/runtime-info.get-fingerprint` and returns the host's
    runtime name. Round-tripping this proves imports, exports, and
    bindgen wiring are aligned.
  - `digest(bytes)` — runs `compose_core::blobs::compute_digest` inside
    the wasm sandbox, proving the portable orchestrator logic is
    reachable through the WIT surface.
  - `audit-demo(tenant, count)` — appends `count` audit entries for
    `tenant` through `compose_core::AuditLogger`, backed by the
    composed secure-log component, then verifies the tenant's hash
    chain and returns the head sequence number. Proves tamper-evident
    audit works end-to-end.
- **`sys:compose/plan@1.0.0`** — `serialize`, `deserialize`,
  `compute-digest`, and `validate`. `validate` needs the blob CAS: the
  host MUST preopen a host-side blobs directory at the guest path
  `/blobs` (via `wasi:filesystem`) before calling it. See the module
  docs in `src/lib.rs` for the full preopen contract.
- **`sys:compose/blobs@1.0.0`** — `put`, `get`, `has`, `size`,
  `delete`, `list-all`. Thin bridge over
  `compose_core::blobs::BlobStore` at the `/blobs` preopen. Same
  preopen requirement as `plan.validate`. `has`, `size`, and
  `list-all` have no error channel in the WIT, so a missing preopen
  degrades to "empty store" for those probes; `put` / `get` / `delete`
  surface it as `internal-error`.
- **`sys:compose/emit@1.0.0`** — `compose`, `get-artifact`,
  `check-cache`. Bridges to `compose_core::emit::EmitHandler` over
  `/blobs` (component + composed-artifact CAS) and `/emit-cache`
  (emit-key → composed-digest lookup). Both preopens are required. On
  wasip2 the emit path always uses the `wac-graph` library route
  because the `wac plug` CLI subprocess check fails.
- **`sys:compose/trust@1.0.0`** — `verify`, `verify-digest`,
  `is-trusted`, `trust-digest`, `untrust-digest`. Bridges to
  `compose_core::trust::TrustStore` at `/trust` (metadata persistence)
  with `SystemClock` (lowered to `wasi:clocks/wall-clock`) for TTL
  bookkeeping. Only the wasm-clean `dev` backend is registered inside
  the guest; SigStore / PGP / X509 backends live in `trust-backends`
  and would need to be pushed through a WIT import to be reachable
  from here.
- Environment override — `validate` reads `COMPOSE_MAX_BLOB_SIZE`
  through `wasi:cli/environment` per invocation. If unset, the default
  100 MiB cap applies; build tools that need to accept composed
  runtimes larger than that should set it to
  `compose_core::limits::BUILD_TOOL_MAX_BLOB_SIZE` (1 GiB) or more.
  The `blobs.put` and `emit.compose` paths honor the same env var.

Still not exported (native-only in `compose-core` for the moment):

- **`sys:compose/exec@1.0.0`** — needs a wasm runtime inside the
  guest, which is out of scope for this component.
- **`sys:compose/events@1.0.0`** — `compose_core::EventCollector`
  matches the interface shape, but routing events back out through
  the host doesn't buy anything until there is a host-side sink to
  wire them to.

## WIT layout

The `wit/` directory is a working copy of `wit/sys-compose/` and
`wit/compose-host/` from the repo root, plus a local `smoke.wit` and
the `orchestrator` world's extra export lines. The duplication will be
removed once `wit-deps` is wired up; until then, treat the repo-level
`wit/` as the spec source of truth and this directory as a checkout of
it.
