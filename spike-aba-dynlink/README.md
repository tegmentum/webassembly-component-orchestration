# spike-aba-dynlink (vestigial)

This directory held an experimental spike for runtime component linking
("dlopen for wasm components"). The "aba" name is historical — an early
naming for the endpoint/consumer/provider triangle — and does not
correspond to any current concept in the codebase.

The spike graduated to production. Its ideas now live at:

- **WIT surface** — [`wit/compose-dynlink/`](../wit/compose-dynlink/)
  (`package.wit`, `endpoint.wit`, `linker.wit`, `world.wit`).
- **Native host implementation** —
  [`hosts/wasmtime/src/dynlink.rs`](../hosts/wasmtime/src/dynlink.rs),
  which implements the `linker` and `endpoint` interfaces and provides
  the shared `sandbox_engine` / `instantiate_owned` primitives used by
  both `compose:dynlink` and `compose:host`.
- **Browser host implementation** —
  [`hosts/browser/`](../hosts/browser/), which implements
  `compose:dynlink/linker` in JavaScript for the "one provider, many
  guests" case.
- **Example components** —
  [`examples/dynlink-echo-provider/`](../examples/dynlink-echo-provider/),
  [`examples/dynlink-endpoint-consumer/`](../examples/dynlink-endpoint-consumer/),
  [`examples/dynlink-dlopen-guest/`](../examples/dynlink-dlopen-guest/).
- **Tests** —
  [`hosts/wasmtime/tests/runtime_linking.rs`](../hosts/wasmtime/tests/runtime_linking.rs).

What remains in this directory (`Cargo.lock`, `target/` directories,
`aba-provider.wasm`, empty `aba-endpoint/` and `harness/` scaffolds)
is vestigial and can be deleted; nothing in the workspace references
it. Kept in-tree only as a historical breadcrumb.
