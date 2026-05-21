//! Clock capability — wall-clock time for timestamps and TTL evaluation.
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A clock that returns the current wall-clock time.
///
/// In the eventual wasm-orchestrator design, this trait will be satisfied by
/// an imported `wasi:clocks/wall-clock` interface.
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn now_unix_secs(&self) -> u64;

    /// Milliseconds since the Unix epoch.
    fn now_unix_millis(&self) -> u64 {
        self.now_unix_secs().saturating_mul(1_000)
    }
}

/// Shared, thread-safe handle to a clock.
pub type SharedClock = Arc<dyn Clock>;

/// Clock backed by [`std::time::SystemTime`] — the host OS wall clock.
#[derive(Debug, Clone, Default)]
pub struct SystemClock;

impl SystemClock {
    pub fn new() -> Self {
        Self
    }

    /// Construct a `SharedClock` backed by the system wall clock.
    pub fn shared() -> SharedClock {
        Arc::new(Self)
    }
}

impl Clock for SystemClock {
    fn now_unix_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn now_unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Manually advanced clock for deterministic testing.
///
/// Tests that exercise TTL or expiration behavior should construct a
/// [`ManualClock`], pass it as the [`SharedClock`] to whichever subsystem is
/// under test, and call [`ManualClock::advance`] to move time forward —
/// avoiding the race between `thread::sleep` and second-boundary truncation
/// in `SystemClock`.
pub struct ManualClock {
    secs: Mutex<u64>,
}

impl ManualClock {
    /// Create a manual clock fixed at the given second.
    pub fn new(initial_secs: u64) -> Self {
        Self {
            secs: Mutex::new(initial_secs),
        }
    }

    /// Construct a `SharedClock` backed by a fresh `ManualClock`.
    pub fn shared(initial_secs: u64) -> Arc<Self> {
        Arc::new(Self::new(initial_secs))
    }

    /// Advance the clock by `seconds`.
    pub fn advance(&self, seconds: u64) {
        let mut guard = self.secs.lock().unwrap();
        *guard = guard.saturating_add(seconds);
    }

    /// Set the clock to an absolute timestamp.
    pub fn set(&self, secs: u64) {
        *self.secs.lock().unwrap() = secs;
    }
}

impl Clock for ManualClock {
    fn now_unix_secs(&self) -> u64 {
        *self.secs.lock().unwrap()
    }

    fn now_unix_millis(&self) -> u64 {
        self.now_unix_secs().saturating_mul(1_000)
    }
}
