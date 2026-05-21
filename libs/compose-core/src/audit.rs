//! Audit logging with tenant isolation, backed by a tamper-evident
//! secure log.
//!
//! Each tenant is a separate `secure-log` stream; entries are
//! hash-chained per stream, so deletion or modification of any
//! record is detectable via [`AuditLogger::verify_tenant`]. The
//! structured `AuditRecord` is encoded into the entry payload; the
//! secure log adds the integrity envelope (sequence number, chain
//! hash, and — once segments are closed — Merkle root + signed
//! checkpoint).
//!
//! The concrete storage backend is injected by the host: the native
//! wasmtime host uses a SQLite-backed `NativeSecureLog`; tests use the
//! in-memory SQLite store. compose-core itself depends only on the
//! `secure-log` trait surface, which is wasm32-wasip2 clean.
use crate::host::SharedClock;
use crate::types::{Digest, TenantId};
use secure_log::SecureLog;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Shared, thread-safe handle to a secure log backend.
///
/// `SecureLog` is only `Send` (SQLite stores are `!Sync`), so a
/// `Mutex` is required for shared access; `Arc<Mutex<...>>` makes the
/// handle both cloneable and `Sync`.
pub type SharedSecureLog = Arc<Mutex<dyn SecureLog>>;

/// Producer string recorded on every audit entry.
const PRODUCER: &str = "compose";

/// Stream id used for records that carry no tenant.
const DEFAULT_STREAM: &str = "default";

/// Audit record for operations. This is the structured payload
/// carried inside each secure-log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Timestamp in milliseconds since epoch
    pub timestamp: u64,
    /// Operation type (emit, exec, etc.)
    pub operation: String,
    /// Plan digest
    pub plan_digest: String,
    /// Cache key (emit-key or exec-key)
    pub cache_key: String,
    /// Tenant ID if any
    pub tenant_id: Option<TenantId>,
    /// Operation outcome (success/failure)
    pub outcome: String,
    /// Additional context
    pub context: Option<String>,
}

/// Audit logger backed by a tamper-evident secure log.
#[derive(Clone)]
pub struct AuditLogger {
    secure_log: SharedSecureLog,
    clock: SharedClock,
}

impl AuditLogger {
    /// Create a new audit logger over the given secure log backend
    /// and clock.
    pub fn new(secure_log: SharedSecureLog, clock: SharedClock) -> Self {
        Self { secure_log, clock }
    }

    fn now(&self) -> u64 {
        self.clock.now_unix_millis()
    }

    /// Append an audit record as a new entry in its tenant's stream.
    pub fn log(&self, record: AuditRecord) -> anyhow::Result<()> {
        let stream = record
            .tenant_id
            .clone()
            .unwrap_or_else(|| DEFAULT_STREAM.to_string());
        let event_type = record.operation.clone();
        let severity = severity_for(&record.outcome);
        let payload = serde_json::to_vec(&record)?;

        let log = self
            .secure_log
            .lock()
            .map_err(|_| anyhow::anyhow!("secure log mutex poisoned"))?;
        log.append(&stream, &event_type, severity, PRODUCER, &payload)?;
        Ok(())
    }

    /// Log an emit operation
    pub fn log_emit(
        &self,
        plan_digest: &Digest,
        emit_key: &Digest,
        tenant_id: Option<&str>,
        outcome: &str,
    ) -> anyhow::Result<()> {
        self.log(AuditRecord {
            timestamp: self.now(),
            operation: "emit".to_string(),
            plan_digest: hex::encode(plan_digest),
            cache_key: hex::encode(emit_key),
            tenant_id: tenant_id.map(|s| s.to_string()),
            outcome: outcome.to_string(),
            context: None,
        })
    }

    /// Log an exec operation
    pub fn log_exec(
        &self,
        plan_digest: &Digest,
        exec_key: &Digest,
        tenant_id: Option<&str>,
        outcome: &str,
        exit_code: Option<u32>,
    ) -> anyhow::Result<()> {
        let context = exit_code.map(|code| format!("exit_code={}", code));

        self.log(AuditRecord {
            timestamp: self.now(),
            operation: "exec".to_string(),
            plan_digest: hex::encode(plan_digest),
            cache_key: hex::encode(exec_key),
            tenant_id: tenant_id.map(|s| s.to_string()),
            outcome: outcome.to_string(),
            context,
        })
    }

    /// Verify the integrity of a tenant's audit chain end to end.
    /// Returns `Ok(())` if every hash link resolves, or an error
    /// identifying the first broken link. An empty stream verifies
    /// trivially.
    pub fn verify_tenant(&self, tenant_id: Option<&str>) -> anyhow::Result<()> {
        let stream = tenant_id.unwrap_or(DEFAULT_STREAM);
        let log = self
            .secure_log
            .lock()
            .map_err(|_| anyhow::anyhow!("secure log mutex poisoned"))?;
        match log.head(stream)? {
            None => Ok(()),
            Some(head) => {
                log.verify_chain(stream, 1, head)?;
                Ok(())
            }
        }
    }
}

/// Map an audit outcome string to a secure-log severity.
fn severity_for(outcome: &str) -> &'static str {
    let lower = outcome.to_ascii_lowercase();
    // "deni" catches both "denied" and "deny".
    if lower.contains("fail") || lower.contains("deni") || lower.contains("error") {
        "error"
    } else {
        "info"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::SystemClock;
    use secure_log::{CborEncoder, NativeSecureLog};
    use secure_log_sqlite::SqliteSecureLogStore;

    fn in_memory_logger() -> AuditLogger {
        let store = SqliteSecureLogStore::open_in_memory().unwrap();
        let log = NativeSecureLog::new(Box::new(store), Box::new(CborEncoder::new()));
        AuditLogger::new(Arc::new(Mutex::new(log)), SystemClock::shared())
    }

    #[test]
    fn test_audit_logging_appends_and_verifies() {
        let logger = in_memory_logger();
        let digest = vec![0u8; 32];

        logger
            .log_emit(&digest, &digest, Some("tenant-1"), "success")
            .unwrap();

        // The tenant's chain must verify after the append.
        logger.verify_tenant(Some("tenant-1")).unwrap();
    }

    #[test]
    fn test_tenant_isolation() {
        let logger = in_memory_logger();
        let digest = vec![0u8; 32];

        logger
            .log_emit(&digest, &digest, Some("tenant-1"), "success")
            .unwrap();
        logger
            .log_exec(&digest, &digest, Some("tenant-2"), "success", Some(0))
            .unwrap();

        // Each tenant's independent chain verifies.
        logger.verify_tenant(Some("tenant-1")).unwrap();
        logger.verify_tenant(Some("tenant-2")).unwrap();
    }

    #[test]
    fn test_failure_outcome_is_error_severity() {
        assert_eq!(severity_for("failure: trap"), "error");
        assert_eq!(severity_for("denied"), "error");
        assert_eq!(severity_for("success"), "info");
        assert_eq!(severity_for("success (cached)"), "info");
    }
}
