//! Adapter implementing compose-core's `SecureLog` trait over the
//! imported `secure-log:log/log` WIT interface.
//!
//! This is the seam that lets the *same* `compose_core::AuditLogger`
//! run inside the wasm orchestrator as runs natively: natively it sits
//! over `NativeSecureLog` + SQLite; here it sits over a WIT import
//! satisfied (via `wac plug`) by the composed `secure-log-sqlite`
//! component. AuditLogger never knows the difference.
//!
//! Only the methods AuditLogger actually uses — `append`, `head`,
//! `verify_chain` — are wired. The Phase-2 segment / inclusion-proof
//! methods return `NotImplemented`; the audit path never calls them.
use compose_core::secure_log::{
    model::digest_from_vec, AppendResult, EntryFields, InclusionProof, SecureLog,
    SecureLogError, SegmentInfo,
};

// The generated bindings for the imported `secure-log:log/log`
// interface. Path mirrors the WIT package `secure-log:log`.
use crate::secure_log::log::log as wit_log;

/// Zero-sized handle over the imported secure-log component. WIT
/// imports are free functions, so there is no per-instance state.
pub struct WitSecureLog;

impl WitSecureLog {
    /// Open / configure the backing store through the log interface.
    /// MUST be called once before any append. `config` is forwarded
    /// verbatim to the store backend (e.g. ":memory:" or a path).
    pub fn open(config: &str) -> Result<Self, String> {
        wit_log::open(config)?;
        Ok(Self)
    }
}

fn storage_err(e: String) -> SecureLogError {
    SecureLogError::Storage(e)
}

impl SecureLog for WitSecureLog {
    fn append(
        &self,
        stream_id: &str,
        event_type: &str,
        severity: &str,
        producer: &str,
        payload: &[u8],
    ) -> Result<AppendResult, SecureLogError> {
        let r = wit_log::append(stream_id, event_type, severity, producer, payload)
            .map_err(storage_err)?;
        Ok(AppendResult {
            seqno: r.seqno,
            entry_hash: digest_from_vec(r.entry_hash, "wit append-result entry-hash")?,
        })
    }

    fn head(&self, stream_id: &str) -> Result<Option<u64>, SecureLogError> {
        wit_log::head(stream_id).map_err(storage_err)
    }

    fn verify_chain(
        &self,
        stream_id: &str,
        from: u64,
        to: u64,
    ) -> Result<(), SecureLogError> {
        wit_log::verify_chain(stream_id, from, to).map_err(storage_err)
    }

    fn read(&self, _seqno: u64) -> Result<EntryFields, SecureLogError> {
        Err(SecureLogError::NotImplemented)
    }

    fn close_segment(&self, _stream_id: &str) -> Result<SegmentInfo, SecureLogError> {
        Err(SecureLogError::NotImplemented)
    }

    fn list_segments(&self, _stream_id: &str) -> Result<Vec<SegmentInfo>, SecureLogError> {
        Err(SecureLogError::NotImplemented)
    }

    fn read_segment(&self, _segment_id: u64) -> Result<SegmentInfo, SecureLogError> {
        Err(SecureLogError::NotImplemented)
    }

    fn inclusion_proof(&self, _seqno: u64) -> Result<InclusionProof, SecureLogError> {
        Err(SecureLogError::NotImplemented)
    }
}
