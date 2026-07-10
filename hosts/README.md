# Host Runtimes

Reference host implementations of the compositional WebAssembly system.
Each host implements the [`compose:host`](../wit/compose-host) and
[`compose:dynlink`](../wit/compose-dynlink) WIT surfaces on top of a
concrete wasm engine, plus enough of the portable orchestrator
(`compose-core`) to run plans natively where that's how the host is
consumed.

## Directory layout

- `wasmtime/` — the reference native host. Cargo crate
  `compose-host-wasmtime`, wraps the `wasmtime` engine, implements
  every `compose:host` and `compose:dynlink` interface, ships a demo
  `main.rs` binary, and is what `composectl` and the conformance runner
  are built against. See [`wasmtime/README.md`](wasmtime/README.md).
- `browser/` — headless browser host. jco-transpiles guest and provider
  components and runs them under WASI Preview 2 via
  `@tegmentum/wasi-polyfill`, with `compose:dynlink/linker` implemented
  in JS. Scope today is limited to the `compose:dynlink` surface —
  proving one shared provider serves many guests. Playwright-driven.
  See [`browser/README.md`](browser/README.md).

Host-specific state (trust roots, cached artifacts, secrets) is not
committed to the repository. `hosts/wasmtime` writes runtime state under
`.compose/{blobs,cache,trust,audit}/` in the working directory by
default; see `HostConfig` in the wasmtime crate.
