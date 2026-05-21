//! Host capability traits.
//!
//! These traits define the surface that the orchestrator core needs from
//! the surrounding runtime — specifically, the things that don't already
//! have a WASI analog and therefore can't be satisfied implicitly by the
//! wasm target.
//!
//! Today the only such surface is the wall clock (used for timestamps,
//! TTL evaluation, and deterministic tests). Everything else compose-core
//! needs from its environment — filesystem (blob CAS, audit log, emit
//! cache, trust metadata), randomness, network — arrives via standard
//! WASI imports when the orchestrator compiles to wasm32-wasip2.
pub mod clock;

pub use clock::{Clock, ManualClock, SharedClock, SystemClock};
