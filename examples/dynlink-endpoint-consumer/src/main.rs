//! A CLI consumer whose `compose:dynlink/endpoint` import is satisfied
//! at exec time by a host-resolved provider (flavor A: late-bound plan
//! imports).
//!
//! The consumer doesn't know or care which component backs the endpoint;
//! it just sends a message and prints the reply. When run by the host
//! with the echo provider bound, `upper` uppercases the payload, so this
//! prints `HELLO FROM CONSUMER`.
wit_bindgen::generate!({
    world: "endpoint-consumer",
    path: "wit",
    generate_all,
});

use compose::dynlink::endpoint::handle;

fn main() {
    match handle("upper", b"hello from consumer") {
        Ok(bytes) => println!("{}", String::from_utf8_lossy(&bytes)),
        Err(e) => {
            eprintln!("endpoint error: {}", e.message);
            std::process::exit(1);
        }
    }
}
