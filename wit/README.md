# WIT Packages

This directory hosts the composable WebAssembly interface definitions used across the project. Each subfolder maps to a published WIT package with canonicalized IDs and deterministic CBOR bundles.

- `canon-wit/` and `canon-cbor/` define the canonicalization interfaces.
- `sys-compose/` contains the core planning, emit, and exec worlds.
- `std-*` packages provide optional runtime worlds (secrets, metrics, audit, attest).

Run `wit-bindgen` or `wasm-tools component wit` against these packages only after updating `SPEC.md` and regenerating canonical hashes.
