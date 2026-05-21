//! Host capability traits.
//!
//! These traits define the surface that the orchestrator core needs from
//! the surrounding runtime. Today every consumer is a Rust struct holding an
//! `Arc<dyn Trait>`; when the orchestrator eventually moves into a wasm
//! component, these same traits become its imported WIT world.
//!
//! Each trait here represents one well-bounded capability:
//!
//! - [`Clock`] — wall-clock time, for timestamps and TTL evaluation.
//! - [`BlobStorage`] — content-addressed storage for components and plans.
//!
//! Concrete implementations live alongside the trait or in host crates.
pub mod blob_storage;
pub mod clock;

pub use blob_storage::{BlobStorage, SharedBlobs};
pub use clock::{Clock, ManualClock, SharedClock, SystemClock};
