//! A minimal plain WASI CLI component (imports only WASI, exports
//! `wasi:cli/run`) used to exercise the `compose:host/runner` capability:
//! the host runs it with args/env/stdin and captured stdio under limits.
//!
//! - default: prints a greeting and echoes the first arg (if any)
//! - `spin`: never returns (used to test CPU/wall-clock limit enforcement)
fn main() {
    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some("spin") {
        let mut n: u64 = 0;
        loop {
            n = core::hint::black_box(n.wrapping_add(1));
        }
    }
    println!("hello from runner");
    if let Some(a) = arg {
        println!("arg: {a}");
    }
}
