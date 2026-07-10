# Wasmtime Host

Reference native host runtime for the compositional WebAssembly system.
This crate is `compose-host-wasmtime`: the wasmtime-specific glue around
the portable [`compose-core`](../../libs/compose-core) crate.

## What's in `src/`

- `lib.rs` — `CompositorHost`, `HostConfig`, and the wasmtime `Engine`
  setup. Constructs the blob CAS, event collector, secrets manager,
  policy enforcer, SQLite-backed audit logger, metrics collector, trust
  store, and attestation service. Re-exports the portable `compose_core`
  API so downstream callers (`composectl`, the conformance runner) can
  depend on this crate directly.
- `exec.rs` — `ExecHandler` and `Mount`. Runs `wasi:cli/run` components
  from a validated plan, wires host-to-guest filesystem preopens, and
  folds runtime-linked providers into the exec-key.
- `compose_host.rs` — host-side implementation of the
  [`compose:host`](../../wit/compose-host) WIT package (`runner`,
  `invoker`, `runtime-info`) via `wasmtime::component::bindgen!`. Loads
  and drives the wasm orchestrator component from
  [`libs/compose-orchestrator-wasm`](../../libs/compose-orchestrator-wasm).
- `dynlink.rs` — host-side implementation of the
  [`compose:dynlink`](../../wit/compose-dynlink) WIT package (`linker`,
  `endpoint`) plus the shared instantiation primitives
  (`instantiate_owned`, `run_cli_command`, `run_cli_with_endpoint`,
  `run_cli_dlopen`, `sandbox_engine`) used by both `compose:host` and
  the exec path. Each provider runs in its own store on a shared fuel +
  epoch engine so memory / CPU / wall-clock limits are enforced.
- `cbor_val.rs` — type-directed CBOR &lt;-&gt; `wasmtime::component::Val`
  marshalling behind `compose:host/invoker.call-with-cbor`.
- `http.rs` — `handle-http` (one request) and `serve-http` (long-running
  server) for `wasi:http/incoming-handler` guests. Gated behind the
  `http-server` feature; uses its own async-enabled engine and a tokio
  runtime.
- `pkcs11_signer.rs` — attestation signer that drives a composed
  `keys:keystore` (softhsm) component so the private key stays inside
  the wasm sandbox. Implements `compose_core::host::Signer`.
- `pkcs11_backend.rs` — PKCS#11 secret backend on top of the
  `pkcs11-host-adapter` WIT surface. Gated behind the `pkcs11` feature.
- `main.rs` — demo binary. See below.

## Build

```
cargo build -p compose-host-wasmtime
```

Feature flags:

- `http-server` — pulls in `wasmtime-wasi-http`, `hyper`, `tokio` and
  enables `http.rs` plus `handle-http` / `serve-http`.
- `pkcs11` — enables `pkcs11_backend.rs` and its `pkcs11-host-adapter`
  dependency.

Neither is on by default.

## `main.rs` — demo binary

The binary is demo/exercise quality, not a supported CLI. It builds a
`CompositorHost` with `HostConfig::default()` (state under
`.compose/{blobs,cache,trust,audit}/` in the cwd) and runs eight demos
in sequence, each logged via `tracing`:

1. plan validation over a minimal test component;
2. secret resolution via the built-in dev backend;
3. trust verification (dev backend, then `trust_digest`);
4. policy enforcement — required / optional capability filtering and
   resource-limit clamping;
5. tenant isolation — two identical plans differing only in
   `policy.tenant` produce distinct `exec-key`s;
6. audit logging — emits entries for three tenants and lists the
   resulting log files;
7. metrics collection — dumps counts and duration summaries;
8. attestation — signs an ed25519 claim with the dev seed, verifies it,
   exports to JSON and SLSA in-toto.

Run it:

```
cargo run -p compose-host-wasmtime
```

For anything past a demo, drive `CompositorHost` from an embedder — the
demos in `main.rs` cover the surface the way a smoke test would.

## Tests

```
cargo test -p compose-host-wasmtime
```

- `orchestrator_smoke.rs` — loads the composed wasm orchestrator and
  exercises `compose:host/smoke.host-name`, `.digest`, `.audit-demo`,
  and the `sys:compose/plan@1.0.0` surface (`serialize`, `deserialize`,
  `compute-digest`, `validate`). Skips if the orchestrator artifact
  hasn't been built (see
  `libs/compose-orchestrator-wasm/build.sh`).
- `runtime_linking.rs` — end-to-end `linkage = Runtime` plan through
  `ExecHandler::run_cli`. Skips if the example provider/consumer
  components aren't built.
- `http_exec.rs` — gated behind `--features http-server`. Skips if
  `examples/hello-http` isn't built.
- `pkcs11_signer.rs` — opt-in. Runs only when `KEYSTORE_TEST_COMPONENT`
  and `KEYSTORE_TEST_CONF` point at a built `keystore-softhsm.wasm` and
  its SoftHSM config.

## Notes

- The default attestation signer uses a fixed dev ed25519 seed. Any
  non-dev deployment MUST override `HostConfig::attest_pkcs11` with a
  `Pkcs11SignerConfig` (see `pkcs11_signer.rs`) or supply another
  `Signer` implementation.
- `HostConfig::default()` caps blobs at 100 MiB (the multi-tenant
  hedge). `HostConfig::build_tool()` raises that to 1 GiB for
  build-time tooling that composes larger runtimes.
