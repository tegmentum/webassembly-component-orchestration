//! The CBOR envelope — the wire contract of `sqlite-extension-endpoint`.
//!
//! This module is the bridging logic written ONCE. It is pure data +
//! (de)serialization; it has NO dependency on `wit_bindgen` so it is
//! shared verbatim across every world shape. Each shape's `lib.rs`
//! converts its bindgen-generated types to/from the types here and
//! routes `endpoint.handle(method, payload)` through `dispatch`.
//!
//! The host forwards bytes verbatim; `method` selects an operation and
//! `payload` is a CBOR-encoded request body (the dynlink "CBOR
//! envelope" convention, mirroring s3-endpoint / gdal-endpoint).
//!
//! ## Method table (the generic envelope dispatch table)
//!
//!   manifest | describe        -> Manifest (full declarative manifest)
//!   policy-check                -> CapabilityReport (reconcile grant)
//!   call                        -> scalar-function.call
//!   agg.step | agg.finalize |
//!   agg.value | agg.inverse     -> aggregate-function lifecycle
//!   collation.compare           -> collation.compare
//!   vtab.<op>                   -> vtab read surface
//!   vtab-update.<op>            -> vtab-update mutating surface
//!   authorizer.authorize        -> authorizer callback
//!   hook.update | hook.commit |
//!   hook.rollback | hook.wal    -> hook callbacks
//!   dotcmd.invoke               -> dot-command invoke
//!
//! A shape only answers the methods its world bridges; everything else
//! (including methods belonging to a tier not in this shape) returns a
//! structured `unsupported`/`unknown method` error. The boundary is
//! defensive: a malformed call decodes to an error, never a panic.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SqlValue — CBOR mirror of sqlite:extension/types.sql-value.
// Tagged so the wire form is self-describing.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "lowercase")]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
    /// The @1.0.0 `wit-value` arm — a structurally-identified,
    /// canonical-CBOR-encoded WIT record. Bridged opaquely: the
    /// provider ferries (type_id, bytes, symbolic_name) without
    /// decoding. (Decoding needs the extension's serde-ops imports —
    /// out of scope for the declarative bridge.)
    WitValue {
        type_id: Vec<u8>,
        bytes: Vec<u8>,
        symbolic_name: String,
    },
}

// ---------------------------------------------------------------------------
// Manifest — the full declarative manifest surfaced over `describe`.
// One CBOR record per registered entry across every tier.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalarSpec {
    pub id: u64,
    pub name: String,
    pub num_args: i32,
    pub deterministic: bool,
    pub direct_only: bool,
    pub innocuous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateSpec {
    pub id: u64,
    pub name: String,
    pub num_args: i32,
    pub is_window: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollationSpec {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabSpec {
    pub id: u64,
    pub name: String,
    pub eponymous: bool,
    pub mutable: bool,
    pub batched: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DotCommandSpec {
    pub id: u64,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub usage: String,
    pub requires_write: bool,
    pub no_args: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub scalars: Vec<ScalarSpec>,
    pub aggregates: Vec<AggregateSpec>,
    pub collations: Vec<CollationSpec>,
    pub vtabs: Vec<VtabSpec>,
    pub dot_commands: Vec<DotCommandSpec>,
    pub has_authorizer: bool,
    pub has_update_hook: bool,
    pub has_commit_hook: bool,
    pub has_wal_hook: bool,
    pub wal_hook_id: u64,
    /// Capabilities the extension REQUIRES (fail-closed gate).
    pub declared_capabilities: Vec<String>,
    /// Capabilities the extension MAY use if granted (not gated).
    pub optional_capabilities: Vec<String>,
}

// ---------------------------------------------------------------------------
// Policy / capability mapping — mirror of the bespoke loader's
// `policy_from_load_options` fail-closed gate, expressed over the
// envelope.
// ---------------------------------------------------------------------------

/// Request to reconcile the extension's declared capabilities against
/// the compose:dynlink plan's granted capability set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyCheckReq {
    /// The grant list from the plan (capability names). Anything the
    /// extension DECLARES that is missing here fails the load.
    pub grant: Vec<String>,
}

/// Result of the reconcile. `ok = false` means FAIL-CLOSED: the host
/// must refuse to register this extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityReport {
    pub ok: bool,
    /// Required-but-not-granted capabilities (non-empty => ok = false).
    pub missing: Vec<String>,
    /// Optional capabilities present in the manifest but absent from the
    /// grant — informational; does NOT fail the load.
    pub optional_ungranted: Vec<String>,
    /// The full granted set the host should apply to this extension.
    pub granted: Vec<String>,
}

impl Manifest {
    /// The describe->register reconciliation + policy/capability gate.
    /// This is the step the bespoke host-side extension-loader performs
    /// (`policy_from_load_options`): every REQUIRED capability must be in
    /// the grant, else fail-closed.
    pub fn reconcile_policy(&self, grant: &[String]) -> CapabilityReport {
        let missing: Vec<String> = self
            .declared_capabilities
            .iter()
            .filter(|c| !grant.contains(c))
            .cloned()
            .collect();
        let optional_ungranted: Vec<String> = self
            .optional_capabilities
            .iter()
            .filter(|c| !grant.contains(c))
            .cloned()
            .collect();
        CapabilityReport {
            ok: missing.is_empty(),
            missing,
            optional_ungranted,
            granted: grant.to_vec(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-tier request envelopes.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallReq {
    pub func_id: u64,
    pub args: Vec<SqlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggStepReq {
    pub func_id: u64,
    pub context_id: u64,
    pub args: Vec<SqlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggCtxReq {
    pub func_id: u64,
    pub context_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollationCompareReq {
    pub collation_id: u64,
    pub a: String,
    pub b: String,
}

// --- vtab read surface ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabConnectReq {
    pub vtab_id: u64,
    pub instance_id: u64,
    pub db_name: String,
    pub table_name: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabInstanceReq {
    pub vtab_id: u64,
    pub instance_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabConstraint {
    pub column: i32,
    /// constraint-op as its WIT discriminant name (eq/gt/le/...).
    pub op: String,
    pub usable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabOrderby {
    pub column: i32,
    pub desc: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabBestIndexReq {
    pub vtab_id: u64,
    pub instance_id: u64,
    pub constraints: Vec<VtabConstraint>,
    pub orderbys: Vec<VtabOrderby>,
    pub col_used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabConstraintUsage {
    pub argv_index: i32,
    pub omit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabIndexPlan {
    pub constraint_usage: Vec<VtabConstraintUsage>,
    pub idx_num: i32,
    pub idx_str: Option<String>,
    pub estimated_cost: f64,
    pub estimated_rows: i64,
    pub orderby_consumed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabOpenReq {
    pub vtab_id: u64,
    pub instance_id: u64,
    pub cursor_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabCursorReq {
    pub vtab_id: u64,
    pub cursor_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabFilterReq {
    pub vtab_id: u64,
    pub cursor_id: u64,
    pub idx_num: i32,
    pub idx_str: Option<String>,
    pub args: Vec<SqlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabColumnReq {
    pub vtab_id: u64,
    pub cursor_id: u64,
    pub col: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabRow {
    pub rowid: i64,
    pub columns: Vec<SqlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabFetchBatchReq {
    pub vtab_id: u64,
    pub cursor_id: u64,
    pub max_rows: u32,
}

// --- vtab-update surface ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabUpdateReq {
    pub vtab_id: u64,
    pub instance_id: u64,
    pub args: Vec<SqlValue>,
}

/// xRename — sqlite renamed the vtab's backing table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabRenameReq {
    pub vtab_id: u64,
    pub instance_id: u64,
    pub new_name: String,
}

/// xSavepoint / xRelease / xRollbackTo — all carry the savepoint id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabSavepointReq {
    pub vtab_id: u64,
    pub instance_id: u64,
    pub savepoint: i32,
}

/// xShadowName — module-level shadow-table-name probe (no instance).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabShadowNameReq {
    pub vtab_id: u64,
    pub name: String,
}

/// xIntegrity — per-instance integrity check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VtabIntegrityReq {
    pub vtab_id: u64,
    pub instance_id: u64,
    pub schema: String,
    pub table_name: String,
    pub mode_flags: u32,
}

// --- authorizer / hooks ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeReq {
    /// auth-action as its WIT discriminant name (insert/select/...).
    pub action: String,
    pub arg1: Option<String>,
    pub arg2: Option<String>,
    pub database: Option<String>,
    pub trigger: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateHookReq {
    /// update-operation as its WIT discriminant name (insert/update/delete).
    pub operation: String,
    pub database: String,
    pub table: String,
    pub rowid: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalHookReq {
    pub hook_id: u64,
    pub db_name: String,
    pub n_frames_in_wal: u32,
}

// --- dot-command ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DotInvokeReq {
    pub func_id: u64,
    pub args: String,
    pub interactive: bool,
    pub display_mode: String,
    pub bail_on_error: bool,
}

/// One state update from a dot-command's invoke-result. `value` is the
/// SqlValue (CBOR-tagged); the host maps it to the cli's value-json wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    pub key: String,
    pub value: SqlValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DotInvokeResp {
    /// Trailing text returned from invoke-result.text.
    pub text: String,
    pub ok: bool,
    pub exit_code: i32,
    /// Everything the extension streamed via the captured cli-stdout.
    pub stdout: String,
    /// Everything streamed via the captured cli-stderr.
    pub stderr: String,
    /// State updates the cli applies after the command returns (`.nullvalue`,
    /// `.echo`, `.headers`, `.parameter set`, ...). Was dropped pre-fix.
    #[serde(default)]
    pub state_deltas: Vec<StateDelta>,
}
