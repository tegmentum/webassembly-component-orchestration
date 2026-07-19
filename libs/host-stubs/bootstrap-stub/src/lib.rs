//! Stub for `host:bootstrap/bootstrap@0.1.0`.
//!
//! Returns an empty argument list. The composed orchestrator wasm
//! calls this from its `tegmentum-probe` diagnostic and treats an
//! empty return as "no argv-analogue provided" -- which is the
//! correct answer for a plugin embedder that never wires the
//! bootstrap handoff.
//!
//! A real embedder replaces this component in the wac plug chain
//! with one that returns real argv / artifact descriptors.

#[allow(warnings)]
mod bindings;

use bindings::exports::host::bootstrap::bootstrap::Guest;

struct Component;

impl Guest for Component {
    fn args() -> Vec<String> {
        Vec::new()
    }
}

bindings::export!(Component with_types_in bindings);
