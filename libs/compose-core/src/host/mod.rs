//! Host capability traits.
//!
//! These traits define the surface that the orchestrator core needs from
//! the surrounding runtime — specifically, the things that don't already
//! have a WASI analog and therefore can't be satisfied implicitly by the
//! wasm target.
//!
//! Two such surfaces exist today:
//!
//! - [`clock`] — wall-clock time (timestamps, TTL evaluation, and
//!   deterministic tests).
//! - [`signer`] — cryptographic signing for attestation, where the
//!   production provider (PKCS#11 / HSM / TPM) keeps the private key
//!   off-host.
//!
//! Everything else compose-core needs from its environment —
//! filesystem (blob CAS, audit log, emit cache, trust metadata),
//! randomness, network — arrives via standard WASI imports when the
//! orchestrator compiles to wasm32-wasip2.
pub mod clock;
pub mod signer;

pub use clock::{Clock, ManualClock, SharedClock, SystemClock};
pub use signer::{verify_ed25519, SharedSigner, Signer, SignerError, SoftwareSigner, ALG_ED25519};
