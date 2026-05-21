//! Orchestrator-as-wasm proof of concept.
//!
//! This crate compiles to a `wasm32-wasip2` component that imports the
//! `compose:host` capabilities and exposes a single smoke-test export.
//! Its only job today is to validate that the WIT package in
//! `wit/compose-host/` flows through `wit_bindgen` and produces a
//! buildable component — not to be a real orchestrator. The actual
//! `sys:compose` exports backed by `compose-core` come in a later step.
wit_bindgen::generate!({
    path: "wit",
    world: "orchestrator",
});

use exports::compose::host::smoke::Guest;

/// Run a tiny round-trip against the host: ask the host for its
/// fingerprint and return the runtime name. Used by host integration
/// tests to confirm the import wiring is correct.
struct Component;

impl Guest for Component {
    fn host_name() -> String {
        let fp = crate::compose::host::runtime_info::get_fingerprint();
        fp.runtime_name
    }
}

export!(Component);
