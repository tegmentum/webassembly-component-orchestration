//! A minimal `compose:dynlink/endpoint` provider used to exercise the
//! runtime-linking host bridge (Phase 2).
//!
//! It speaks the uniform message endpoint: the host forwards opaque
//! `(method, payload)` bytes and this component returns opaque bytes.
//! The "protocol" here is intentionally trivial — no CBOR envelope —
//! because the point is to prove the host can resolve, instantiate, and
//! call a provider, not to demonstrate a real wire format.
//!
//! Methods:
//!   - `echo`  -> payload unchanged
//!   - `upper` -> payload ASCII-uppercased
//!   - `len`   -> payload length as decimal ASCII
//!   - other   -> `unknown-method:<method>` (still an Ok response)
wit_bindgen::generate!({
    world: "dynlink-provider",
    path: "wit",
    generate_all,
});

use exports::compose::dynlink::endpoint::{Error, Guest};

struct EchoProvider;

impl Guest for EchoProvider {
    fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
        let response = match method.as_str() {
            "echo" => payload,
            "upper" => payload.to_ascii_uppercase(),
            "len" => payload.len().to_string().into_bytes(),
            // Never returns: used by the host to test CPU/fuel and
            // wall-clock (epoch) limit enforcement, which interrupt it.
            "spin" => {
                let mut n: u64 = 0;
                loop {
                    n = core::hint::black_box(n.wrapping_add(1));
                }
            }
            other => format!("unknown-method:{other}").into_bytes(),
        };
        Ok(response)
    }
}

export!(EchoProvider);
