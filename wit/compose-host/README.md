# `compose:host` — host capability for the wasm orchestrator

This package defines the WIT world a native host must implement so the
orchestrator can run as a wasm component itself, rather than as the
embedded Rust library that lives in `hosts/wasmtime/` today.

The deliberate design goal is to keep this surface as small as
possible. Everything that has a standard WASI analog stays out —
filesystem, clocks, randomness, network, stdio. The orchestrator
gets those by importing standard `wasi:*` interfaces, which any
runtime implementing the component model already provides.

What is irreducibly host-side is component instantiation: the
orchestrator needs to run other wasm components on its users'
behalf, and only a native runtime can own a wasm engine. That's
this package's entire reason to exist.

## Interfaces

| Interface         | Purpose                                                                                 |
|-------------------|-----------------------------------------------------------------------------------------|
| `runner`          | Instantiate + run a CLI-style component, capture stdio + exit. The 80% case.           |
| `invoker`         | Long-lived instances with typed export calls. Provisional pending component-model gaps. |
| `runtime-info`    | Host fingerprint + supported imports. Feeds the orchestrator's `exec-key`.              |

Every operation reports failures as a typed `exec-error` variant —
no opaque error strings, no host-specific extension points. A guest
that imports something the host cannot satisfy gets the unsatisfied
import name back; a runtime trap returns the trap message; a limit
breach is distinguished from a host I/O failure.

## Worlds

| World          | Implemented by  | Imports                                                                                                 |
|----------------|-----------------|---------------------------------------------------------------------------------------------------------|
| `host`         | Native runtime  | (none — it provides this world)                                                                         |
| `orchestrator` | Wasm component  | `compose:host` + the standard `wasi:filesystem` / `wasi:clocks` / `wasi:random` / `wasi:io` interfaces  |

The `orchestrator` world re-exports the existing `sys:compose`
interfaces (`plan`, `emit`, `exec`, `blobs`, `trust`, `events`) so
the move from "orchestrator as a Rust crate" to "orchestrator as a
wasm component" is invisible to downstream consumers — `composectl`,
the conformance runner, and any other caller keep talking to
`sys:compose` exactly as before.

## What's deliberately not here

- **Storage interfaces.** The orchestrator's blob CAS / audit log /
  emit cache / trust metadata are all filesystem operations under
  `compose-core`'s `std::fs` calls, which become `wasi:filesystem`
  calls in the wasm target. No bespoke `KeyValueStore` /
  `AuditSink` interface is needed.
- **Clock and randomness.** Covered by `wasi:clocks` and
  `wasi:random`.
- **HTTP.** SigStore / Rekor / Vault and any future registry
  protocols go through `wasi:http/outgoing-handler`.
- **Crypto.** Hashing (`sha2`) and CBOR encoding (`ciborium`) run
  inside the orchestrator wasm. Signature verification for trust
  backends can either run pure-wasm (e.g. `ed25519-dalek`,
  `ring`-equivalents) or, if the host has hardware crypto, import
  a `wasi:keyvalue`-shaped or `wasi:crypto` proposal — both
  expansions belong in the standards track, not this package.

The smaller this package stays, the more substitutable the
orchestrator becomes across hosts. Each new interface added here
is a new conformance burden for every host implementation. Resist.

## Status

Draft. The interfaces compile against `wasm-tools resolve` /
`wit-parser`, but no host has implemented them yet — `hosts/wasmtime`
currently still calls the Rust `compose-core` library directly rather
than going through this WIT surface. Producing a wasm component that
exports `orchestrator` is the next milestone.

The `invoker` interface in particular hinges on a component-model
gap: WIT can express the resource handle and the named-export
lookup, but cannot yet express invocation polymorphic over arbitrary
canonical-ABI value types. Until that gap closes, `call-with-cbor`
is a pragmatic stand-in: both sides agree on a schema-less wire
format. Replace it with structured invocation as soon as the
component model can express it.
