//! A CLI guest that drives runtime linking itself (flavor B): it asks the
//! host's `compose:dynlink/linker` to resolve a provider by id at run
//! time, calls into it, and prints the reply.
//!
//! The plan doesn't bind anything — the guest decides what to load. Run
//! by the host with an echo provider registered under id "provider", this
//! prints `HELLO FROM DLOPEN`.
wit_bindgen::generate!({
    world: "dynlink-guest",
    path: "wit",
    generate_all,
});

use compose::dynlink::linker::resolve_by_id;

fn main() {
    let instance = match resolve_by_id("provider") {
        Ok(i) => i,
        Err(e) => {
            eprintln!("resolve failed: {}", e.message);
            std::process::exit(1);
        }
    };
    match instance.invoke("upper", b"hello from dlopen") {
        Ok(bytes) => println!("{}", String::from_utf8_lossy(&bytes)),
        Err(e) => {
            eprintln!("invoke failed: {}", e.message);
            std::process::exit(1);
        }
    }
}
