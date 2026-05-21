/// Audit logging with tenant isolation
use crate::host::{SharedClock, SystemClock};
use crate::types::{Digest, TenantId};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// Audit record for operations
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

/// Audit logger
#[derive(Clone)]
pub struct AuditLogger {
    log_dir: PathBuf,
    clock: SharedClock,
}

impl AuditLogger {
    /// Create a new audit logger backed by the given clock.
    pub fn new(log_dir: PathBuf, clock: SharedClock) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&log_dir)?;
        Ok(Self { log_dir, clock })
    }

    fn now(&self) -> u64 {
        self.clock.now_unix_millis()
    }

    /// Log an audit record
    pub fn log(&self, record: AuditRecord) -> anyhow::Result<()> {
        // Group by tenant if present, otherwise use "default"
        let tenant_dir = if let Some(tenant_id) = &record.tenant_id {
            self.log_dir.join(tenant_id)
        } else {
            self.log_dir.join("default")
        };

        std::fs::create_dir_all(&tenant_dir)?;

        // Append to daily log file
        let date = chrono::DateTime::from_timestamp(
            (record.timestamp / 1000) as i64,
            0,
        )
        .unwrap_or_else(|| chrono::Utc::now())
        .format("%Y-%m-%d");
        let log_file = tenant_dir.join(format!("audit-{}.jsonl", date));

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)?;

        let json = serde_json::to_string(&record)?;
        writeln!(file, "{}", json)?;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_audit_logging() {
        let dir = tempdir().unwrap();
        let logger = AuditLogger::new(dir.path().to_path_buf(), SystemClock::shared()).unwrap();

        let digest = vec![0u8; 32];
        logger
            .log_emit(&digest, &digest, Some("tenant-1"), "success")
            .unwrap();

        // Check file was created
        let tenant_dir = dir.path().join("tenant-1");
        assert!(tenant_dir.exists());

        // Check log file exists
        let files: Vec<_> = std::fs::read_dir(&tenant_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_tenant_isolation() {
        let dir = tempdir().unwrap();
        let logger = AuditLogger::new(dir.path().to_path_buf(), SystemClock::shared()).unwrap();

        let digest = vec![0u8; 32];
        logger
            .log_emit(&digest, &digest, Some("tenant-1"), "success")
            .unwrap();
        logger
            .log_emit(&digest, &digest, Some("tenant-2"), "success")
            .unwrap();

        // Check separate directories
        assert!(dir.path().join("tenant-1").exists());
        assert!(dir.path().join("tenant-2").exists());
    }
}
