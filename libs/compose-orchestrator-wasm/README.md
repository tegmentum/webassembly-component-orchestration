# `compose-orchestrator-wasm`

Proof-of-concept wasm component that consumes the `compose:host` WIT
package. Its only job today is to validate that the WIT in
`wit/compose-host/` flows through `wit-bindgen`, compiles to
`wasm32-wasip2`, and produces a component whose declared world matches
the spec.

This crate is **not** part of the workspace (it's listed in the root
`Cargo.toml`'s `exclude`). Build it on its own:

```bash
./build.sh
```

That produces `target/wasm32-wasip2/release/compose_orchestrator_wasm.wasm`
(~16KB) and, if `wasm-tools` is on `$PATH`, prints the resulting WIT
world.

## Status

Smoke-test only. Exports a single `compose:host/smoke.host-name`
function that calls into `runtime-info.get-fingerprint` and returns
the host's reported runtime name. Used by future host integration
tests to confirm the import wiring is correct.

The real orchestrator content — `compose-core`'s plan / emit / exec /
blobs / trust / events logic, behind `sys:compose@1.0.0` exports —
comes in a later step, after the WASI dependency story is sorted
(probably via `wit-deps`).

## WIT layout

The `wit/` directory is currently a working copy of `wit/compose-host/`
from the repo root, plus a local `smoke.wit` and a tweak to the
`orchestrator` world that adds `export smoke`. The duplication will be
removed once `wit-deps` is wired up; until then, treat the repo-level
`wit/compose-host/` as the spec source of truth and this directory as
a checkout of it.
