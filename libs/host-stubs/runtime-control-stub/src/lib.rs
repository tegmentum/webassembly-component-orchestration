//! Stub for `tegmentum:runtime/control@0.1.0`.
//!
//! Provides the `runtime` resource with a no-op constructor. The
//! composed orchestrator wasm never actually invokes this at
//! runtime -- the reference is a function-pointer probe under
//! `core::hint::black_box` in smoke.tegmentum-probe so LLVM keeps
//! the extern alive for `wasm-tools component wit` visibility.
//! wac plug still needs the corresponding export to link the
//! composed component.
//!
//! A real embedder that ships a backend adapter (wasmtime, WAMR,
//! WasmEdge) exports the same interface with real behaviour and
//! replaces this component in the wac plug chain.

#[allow(warnings)]
mod bindings;

use bindings::exports::tegmentum::runtime::control::{Guest, GuestRuntime, ResourceLimits};

struct Component;

impl Guest for Component {
    type Runtime = StubRuntime;
}

pub struct StubRuntime;

impl GuestRuntime for StubRuntime {
    fn new(_limits: ResourceLimits) -> Self {
        StubRuntime
    }
}

bindings::export!(Component with_types_in bindings);
