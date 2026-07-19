//! Stub for `compose:host/runtime-info@0.1.0`.
//!
//! Returns a fixed fingerprint that identifies this build as the
//! stardog/jvm plugin path. The values here are deliberately static
//! (not read from the environment) so the composed orchestrator
//! reports a stable identity regardless of the outer embedder --
//! this is a stub, not a real runtime probe.
//!
//! Consumers who need a real fingerprint replace this component in
//! the `wac plug` chain with one that reads real host data.

#[allow(warnings)]
mod bindings;

use bindings::exports::compose::host::runtime_info::{Fingerprint, Guest};

struct Component;

impl Guest for Component {
    fn get_fingerprint() -> Fingerprint {
        Fingerprint {
            runtime_name: "stardog".to_string(),
            runtime_version: "1.0.0".to_string(),
            engine_features_hash: Vec::new(),
            host_target: "jvm".to_string(),
        }
    }
}

bindings::export!(Component with_types_in bindings);
