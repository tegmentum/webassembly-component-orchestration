# Browser Host

The first **browser** host for the orchestration framework, parallel to
`hosts/wasmtime/`. It proves `compose:dynlink` works in a real (headless)
browser: a jco-transpiled **guest** resolves a **provider** through a
JS-implemented `compose:dynlink/linker` and invokes it, running under
WASI Preview 2 via [`@tegmentum/wasi-polyfill`]. The provider is
instantiated **once** and shared across every resolve — the "one copy for
many" composition strategy that underpins ducklink's browser story (one
heavy provider, many guests).

## What it does

- `examples/dynlink-dlopen-guest` — a `wasi:cli/run` CLI that imports
  `compose:dynlink/linker`, calls `resolve_by_id("provider")`, then
  `instance.invoke("upper", b"hello from dlopen")`, and prints the result.
- `examples/dynlink-echo-provider` — exports `compose:dynlink/endpoint`
  with `handle(method, payload)`; `"upper"` uppercases the payload.

Driven through the browser host, the guest prints `HELLO FROM DLOPEN`.

## The wiring (`src/dynlink-host.js`)

1. **Provider instantiated once.** The provider is transpiled in default
   mode; its emitted JS self-instantiates at module-evaluation time
   (top-level `await $init`). A single memoised `import()` therefore
   yields one live provider instance whose
   `endpoint.handle(method, payload) -> Uint8Array` is the shared compute
   surface. We count the import to assert "instantiated once".

2. **`compose:dynlink/linker` implemented in JS.** jco shapes this import
   as `{ Instance, resolveById, resolveByDigest }`. The guest's generated
   trampoline does `Object.create(Instance.prototype)` /
   `e instanceof Instance`, then calls `instance.invoke(method, payload)`.
   So the host supplies an `Instance` **class** whose `invoke` dispatches
   straight into the shared `endpoint.handle`. `resolveById` returns a new
   `Instance` handle per call, but **all handles close over the one
   provider** — that is the sharing proof.

3. **WASI from the polyfill** (`src/host-imports.js`): mirrors sqlink's
   `buildCliHostImports` — `createPolyfill` / `createPolicy` from
   `@tegmentum/wasi-polyfill/wasip2` plus the random / clocks / io / cli
   plugins, jcoCompat un-versioned import names. stdout is captured via
   the `stdout` plugin's `onStdout` callback so the harness reads the
   printed result.

4. **Drive `wasi:cli/run#run`.** The guest is async-instantiation mode:
   `instantiate(getCoreModule, imports, instantiateCore) -> { run }`.
   `getCoreModule` fetches + compiles the embedded `*.core.wasm`. The host
   runs the guest, then `runSharedDemo()` runs it **twice** — both runs
   resolve + invoke through the linker, both hit the single provider.

## Engine / JSPI

- **Engine:** Playwright-bundled **Chromium 149** (`@playwright/test`).
- **JSPI:** enabled **by default** (`WebAssembly.Suspending` /
  `WebAssembly.promising` are present with no launch flag — verified by
  `tests/jspi-probe.spec.js`). CLI components can suspend on async WASI
  imports (`wasi:io/streams` blocking ops, `wasi:io/poll.block`); under
  Chromium 137+ JSPI handles those. If a CI runner pins chromium below
  137, add `--js-flags=--experimental-wasm-jspi` under
  `use.launchOptions.args` in `playwright.config.js`.

## Run

```sh
npm install
npm run test:install   # one-time: playwright chromium
npm test               # playwright: dynlink.spec.js + jspi-probe.spec.js
```

`npm run transpile` re-runs jco (1.15.x) over the prebuilt example
components into `transpiled/` (gitignored): the guest in
`--instantiation async --async-mode jspi` mode, the provider in default
mode. Build the example `.wasm` first (`examples/build-all.sh`).

## Generalising to many guests (the pylon case)

The provider is a plain ES-module singleton: any number of guests can be
instantiated against the **same** `endpoint.handle` closure, each with its
own `Instance` handle from `resolveById`. There is no per-guest provider
state in this host — `invoke` is a pure pass-through — so N guests sharing
one provider is the same code path run N times. The shared-provider demo
(`runSharedDemo`, two runs, `providerImportCount === 1`) is the 2-guest
instance of that general case.

[`@tegmentum/wasi-polyfill`]: ../../../wasi-polyfill
