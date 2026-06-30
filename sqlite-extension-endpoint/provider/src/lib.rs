//! `sqlite-extension-endpoint` — the REUSABLE, parameterized
//! `compose:dynlink/endpoint` provider that bridges DECLARATIVE
//! `sqlite:extension@1.0.0` tiers to the dynlink bytes envelope.
//!
//! Generalizes the spike #218 aba-specific adapter. The bridging logic
//! (the CBOR envelope) lives ONCE in `envelope.rs`; this file binds it
//! to one world SHAPE selected by a Cargo feature. Every shape compiles
//! the same envelope; the SPECIFIC extension is supplied at compose time
//! via `wac plug`, never recompiled into the provider.
//!
//! Build one shape at a time, e.g.:
//!   cargo build --release --target wasm32-wasip2 --no-default-features --features aggregate

mod envelope;
use envelope::*;

// Exactly one shape feature must be active.
#[cfg(not(any(
    feature = "scalar",
    feature = "aggregate",
    feature = "collation",
    feature = "vtab",
    feature = "vtab-mut",
    feature = "hooks",
    feature = "dotcmd",
)))]
compile_error!("select exactly one shape feature: scalar|aggregate|collation|vtab|vtab-mut|hooks|dotcmd");

// ===========================================================================
// Shared encode/decode/error helpers, generic over the bindgen Error type
// each world produces. The error type is identical in shape across worlds
// (compose:dynlink endpoint's `error` = sys:compose/types.error), so each
// shape constructs it through these tiny helpers.
// ===========================================================================

macro_rules! provider_impl {
    ($world:literal) => {
        wit_bindgen::generate!({ world: $world, path: "wit", generate_all });

        use exports::compose::dynlink::endpoint::{Error, Guest};
        use sys::compose::types::ErrorCode;

        fn err(code: ErrorCode, message: String) -> Error {
            Error { code, message, context: None }
        }

        fn encode<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, Error> {
            let mut out = Vec::new();
            ciborium::ser::into_writer(v, &mut out)
                .map_err(|e| err(ErrorCode::InternalError, format!("cbor encode: {e}")))?;
            Ok(out)
        }

        fn decode<T: serde::de::DeserializeOwned>(payload: &[u8]) -> Result<T, Error> {
            ciborium::de::from_reader(payload)
                .map_err(|e| err(ErrorCode::InvalidInput, format!("cbor decode: {e}")))
        }

        fn unknown(method: &str) -> Error {
            err(ErrorCode::InvalidInput, format!("unknown method: {method}"))
        }

        struct Provider;
    };
}

// ===========================================================================
// SqlValue <-> bindgen conversions. Defined per shape because the bindgen
// `SqlValue` is a distinct type per world; the body is identical.
// `bindings_sqlvalue!` is invoked inside each shape module after its
// `generate!` so `sqlite::extension::types` is in scope.
// ===========================================================================

macro_rules! sqlvalue_conv {
    () => {
        use sqlite::extension::types::SqlValue as WitVal;
        use sqlite::extension::types::WitValuePayload;

        fn to_wit(v: &SqlValue) -> WitVal {
            match v {
                SqlValue::Null => WitVal::Null,
                SqlValue::Integer(i) => WitVal::Integer(*i),
                SqlValue::Real(r) => WitVal::Real(*r),
                SqlValue::Text(s) => WitVal::Text(s.clone()),
                SqlValue::Blob(b) => WitVal::Blob(b.clone()),
                SqlValue::WitValue { type_id, bytes, symbolic_name } => {
                    WitVal::WitValue(WitValuePayload {
                        type_id: type_id.clone(),
                        bytes: bytes.clone(),
                        symbolic_name: symbolic_name.clone(),
                    })
                }
            }
        }

        fn from_wit(v: WitVal) -> SqlValue {
            match v {
                WitVal::Null => SqlValue::Null,
                WitVal::Integer(i) => SqlValue::Integer(i),
                WitVal::Real(r) => SqlValue::Real(r),
                WitVal::Text(s) => SqlValue::Text(s),
                WitVal::Blob(b) => SqlValue::Blob(b),
                WitVal::WitValue(p) => SqlValue::WitValue {
                    type_id: p.type_id,
                    bytes: p.bytes,
                    symbolic_name: p.symbolic_name,
                },
            }
        }

        #[allow(dead_code)]
        fn args_to_wit(args: &[SqlValue]) -> Vec<WitVal> {
            args.iter().map(to_wit).collect()
        }
    };
}

// ===========================================================================
// Manifest reconciliation: bindgen manifest -> envelope::Manifest. Shared
// across every shape (metadata is imported by all). Implemented as a macro
// because `metadata::describe()` returns a per-world bindgen type.
// ===========================================================================

macro_rules! describe_manifest {
    () => {{
        use sqlite::extension::metadata as meta;
        use sqlite::extension::policy::Capability;

        fn flag(f: meta::FunctionFlags, want: meta::FunctionFlags) -> bool {
            f.contains(want)
        }

        fn cap_name(c: &Capability) -> String {
            // Stable string names mirroring the policy.wit `capability`
            // variant — the same identifiers the plan's grant list uses.
            match c {
                Capability::Spi => "spi",
                Capability::Prepared => "prepared",
                Capability::Transaction => "transaction",
                Capability::Schema => "schema",
                Capability::State => "state",
                Capability::Cache => "cache",
                Capability::Random => "random",
                Capability::Text => "text",
                Capability::Hashing => "hashing",
                Capability::Encoding => "encoding",
                Capability::Http => "http",
                Capability::Dns => "dns",
                Capability::WalFrames => "wal-frames",
                Capability::S3 => "s3",
                Capability::SpawnBuild => "spawn-build",
                Capability::Bundles => "bundles",
            }
            .to_string()
        }

        let m = meta::describe();
        Manifest {
            name: m.name,
            version: m.version,
            scalars: m
                .scalar_functions
                .iter()
                .map(|s| ScalarSpec {
                    id: s.id,
                    name: s.name.clone(),
                    num_args: s.num_args,
                    deterministic: flag(s.func_flags, meta::FunctionFlags::DETERMINISTIC),
                    direct_only: flag(s.func_flags, meta::FunctionFlags::DIRECT_ONLY),
                    innocuous: flag(s.func_flags, meta::FunctionFlags::INNOCUOUS),
                })
                .collect(),
            aggregates: m
                .aggregate_functions
                .iter()
                .map(|a| AggregateSpec {
                    id: a.id,
                    name: a.name.clone(),
                    num_args: a.num_args,
                    is_window: a.is_window,
                })
                .collect(),
            collations: m
                .collations
                .iter()
                .map(|c| CollationSpec { id: c.id, name: c.name.clone() })
                .collect(),
            vtabs: m
                .vtabs
                .iter()
                .map(|v| VtabSpec {
                    id: v.id,
                    name: v.name.clone(),
                    eponymous: v.eponymous,
                    mutable: v.mutable,
                    batched: v.batched,
                })
                .collect(),
            dot_commands: m
                .dot_commands
                .iter()
                .map(|d| DotCommandSpec {
                    id: d.id,
                    name: d.name.clone(),
                    version: d.version.clone(),
                    summary: d.summary.clone(),
                    usage: d.usage.clone(),
                    requires_write: d.requires_write,
                    no_args: d.no_args,
                })
                .collect(),
            has_authorizer: m.has_authorizer,
            has_update_hook: m.has_update_hook,
            has_commit_hook: m.has_commit_hook,
            has_wal_hook: m.has_wal_hook,
            wal_hook_id: m.wal_hook_id,
            declared_capabilities: m.declared_capabilities.iter().map(cap_name).collect(),
            optional_capabilities: m.optional_capabilities.iter().map(cap_name).collect(),
        }
    }};
}

// The `manifest`/`describe`/`policy-check` methods are common to every
// shape; this macro emits the shared match arms.
macro_rules! common_methods {
    ($method:expr, $payload:expr) => {
        match $method {
            "manifest" | "describe" => return encode(&describe_manifest!()),
            "policy-check" => {
                let req: PolicyCheckReq = decode($payload)?;
                let manifest = describe_manifest!();
                return encode(&manifest.reconcile_policy(&req.grant));
            }
            _ => {}
        }
    };
}

// ===========================================================================
// SHAPE: scalar  (world provider-scalar)
// ===========================================================================
#[cfg(feature = "scalar")]
mod shape {
    use super::envelope::*;
    provider_impl!("provider-scalar");
    sqlvalue_conv!();

    impl Guest for Provider {
        fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
            common_methods!(method.as_str(), &payload);
            match method.as_str() {
                "call" => {
                    let req: CallReq = decode(&payload)?;
                    match sqlite::extension::scalar_function::call(req.func_id, &args_to_wit(&req.args)) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("scalar call: {m}"))),
                    }
                }
                other => Err(unknown(other)),
            }
        }
    }
    export!(Provider);
}

// ===========================================================================
// SHAPE: aggregate  (world provider-aggregate)
// ===========================================================================
#[cfg(feature = "aggregate")]
mod shape {
    use super::envelope::*;
    provider_impl!("provider-aggregate");
    sqlvalue_conv!();
    use sqlite::extension::aggregate_function as agg;

    impl Guest for Provider {
        fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
            common_methods!(method.as_str(), &payload);
            match method.as_str() {
                "call" => {
                    let req: CallReq = decode(&payload)?;
                    match sqlite::extension::scalar_function::call(req.func_id, &args_to_wit(&req.args)) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("scalar call: {m}"))),
                    }
                }
                "agg.step" => {
                    let r: AggStepReq = decode(&payload)?;
                    match agg::step(r.func_id, r.context_id, &args_to_wit(&r.args)) {
                        Ok(()) => encode(&()),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("agg.step: {m}"))),
                    }
                }
                "agg.finalize" => {
                    let r: AggCtxReq = decode(&payload)?;
                    match agg::finalize(r.func_id, r.context_id) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("agg.finalize: {m}"))),
                    }
                }
                "agg.value" => {
                    let r: AggCtxReq = decode(&payload)?;
                    match agg::value(r.func_id, r.context_id) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("agg.value: {m}"))),
                    }
                }
                "agg.inverse" => {
                    let r: AggStepReq = decode(&payload)?;
                    match agg::inverse(r.func_id, r.context_id, &args_to_wit(&r.args)) {
                        Ok(()) => encode(&()),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("agg.inverse: {m}"))),
                    }
                }
                other => Err(unknown(other)),
            }
        }
    }
    export!(Provider);
}

// ===========================================================================
// SHAPE: collation  (world provider-collation)
// ===========================================================================
#[cfg(feature = "collation")]
mod shape {
    use super::envelope::*;
    provider_impl!("provider-collation");
    sqlvalue_conv!();

    impl Guest for Provider {
        fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
            common_methods!(method.as_str(), &payload);
            match method.as_str() {
                "call" => {
                    let req: CallReq = decode(&payload)?;
                    match sqlite::extension::scalar_function::call(req.func_id, &args_to_wit(&req.args)) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("scalar call: {m}"))),
                    }
                }
                "collation.compare" => {
                    let r: CollationCompareReq = decode(&payload)?;
                    let cmp = sqlite::extension::collation::compare(r.collation_id, &r.a, &r.b);
                    encode(&cmp)
                }
                other => Err(unknown(other)),
            }
        }
    }
    export!(Provider);
}

// ===========================================================================
// SHAPE: vtab  (world provider-vtab) — read-only declarative surface
// ===========================================================================
#[cfg(feature = "vtab")]
mod shape {
    use super::envelope::*;
    provider_impl!("provider-vtab");
    sqlvalue_conv!();
    use sqlite::extension::vtab as vt;

    fn op_name(op: vt::ConstraintOp) -> String {
        use vt::ConstraintOp::*;
        match op {
            Eq => "eq", Gt => "gt", Le => "le", Lt => "lt", Ge => "ge", Ne => "ne",
            Match => "match", Like => "like", Regexp => "regexp", Glob => "glob",
            IsNull => "is-null", IsNotNull => "is-not-null", Limit => "limit",
            Offset => "offset", Function => "function",
        }
        .to_string()
    }

    impl Guest for Provider {
        fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
            common_methods!(method.as_str(), &payload);
            match method.as_str() {
                "call" => {
                    let req: CallReq = decode(&payload)?;
                    match sqlite::extension::scalar_function::call(req.func_id, &args_to_wit(&req.args)) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("scalar call: {m}"))),
                    }
                }
                "vtab.connect" | "vtab.create" => {
                    let r: VtabConnectReq = decode(&payload)?;
                    let res = if method == "vtab.create" {
                        vt::create(r.vtab_id, r.instance_id, &r.db_name, &r.table_name, &r.args)
                    } else {
                        vt::connect(r.vtab_id, r.instance_id, &r.db_name, &r.table_name, &r.args)
                    };
                    match res {
                        Ok(schema) => encode(&schema),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("vtab.connect: {m}"))),
                    }
                }
                "vtab.best-index" => {
                    let r: VtabBestIndexReq = decode(&payload)?;
                    let constraints: Vec<vt::Constraint> = r
                        .constraints
                        .iter()
                        .map(|c| vt::Constraint {
                            column: c.column,
                            op: parse_op(&c.op),
                            usable: c.usable,
                        })
                        .collect();
                    let orderbys: Vec<vt::Orderby> = r
                        .orderbys
                        .iter()
                        .map(|o| vt::Orderby { column: o.column, desc: o.desc })
                        .collect();
                    let info = vt::IndexInfo { constraints, orderbys, col_used: r.col_used };
                    match vt::best_index(r.vtab_id, r.instance_id, &info) {
                        Ok(p) => encode(&VtabIndexPlan {
                            constraint_usage: p
                                .constraint_usage
                                .iter()
                                .map(|u| VtabConstraintUsage { argv_index: u.argv_index, omit: u.omit })
                                .collect(),
                            idx_num: p.idx_num,
                            idx_str: p.idx_str,
                            estimated_cost: p.estimated_cost,
                            estimated_rows: p.estimated_rows,
                            orderby_consumed: p.orderby_consumed,
                        }),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("vtab.best-index: {m}"))),
                    }
                }
                "vtab.open" => {
                    let r: VtabOpenReq = decode(&payload)?;
                    vt::open(r.vtab_id, r.instance_id, r.cursor_id)
                        .map(|_| Vec::new())
                        .map_err(|m| err(ErrorCode::ExecTrap, format!("vtab.open: {m}")))
                }
                "vtab.filter" => {
                    let r: VtabFilterReq = decode(&payload)?;
                    vt::filter(r.vtab_id, r.cursor_id, r.idx_num, r.idx_str.as_deref(), &args_to_wit(&r.args))
                        .map(|_| Vec::new())
                        .map_err(|m| err(ErrorCode::ExecTrap, format!("vtab.filter: {m}")))
                }
                "vtab.next" => {
                    let r: VtabCursorReq = decode(&payload)?;
                    vt::next(r.vtab_id, r.cursor_id)
                        .map(|_| Vec::new())
                        .map_err(|m| err(ErrorCode::ExecTrap, format!("vtab.next: {m}")))
                }
                "vtab.eof" => {
                    let r: VtabCursorReq = decode(&payload)?;
                    encode(&vt::eof(r.vtab_id, r.cursor_id))
                }
                "vtab.column" => {
                    let r: VtabColumnReq = decode(&payload)?;
                    match vt::column(r.vtab_id, r.cursor_id, r.col) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("vtab.column: {m}"))),
                    }
                }
                "vtab.rowid" => {
                    let r: VtabCursorReq = decode(&payload)?;
                    match vt::rowid(r.vtab_id, r.cursor_id) {
                        Ok(v) => encode(&v),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("vtab.rowid: {m}"))),
                    }
                }
                "vtab.fetch-batch" => {
                    let r: VtabFetchBatchReq = decode(&payload)?;
                    match vt::fetch_batch(r.vtab_id, r.cursor_id, r.max_rows) {
                        Ok(rows) => encode(
                            &rows
                                .into_iter()
                                .map(|row| VtabRow {
                                    rowid: row.rowid,
                                    columns: row.columns.into_iter().map(from_wit).collect(),
                                })
                                .collect::<Vec<_>>(),
                        ),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("vtab.fetch-batch: {m}"))),
                    }
                }
                "vtab.close" => {
                    let r: VtabCursorReq = decode(&payload)?;
                    vt::close(r.vtab_id, r.cursor_id)
                        .map(|_| Vec::new())
                        .map_err(|m| err(ErrorCode::ExecTrap, format!("vtab.close: {m}")))
                }
                "vtab.disconnect" => {
                    let r: VtabInstanceReq = decode(&payload)?;
                    vt::disconnect(r.vtab_id, r.instance_id)
                        .map(|_| Vec::new())
                        .map_err(|m| err(ErrorCode::ExecTrap, format!("vtab.disconnect: {m}")))
                }
                "vtab.destroy" => {
                    let r: VtabInstanceReq = decode(&payload)?;
                    vt::destroy(r.vtab_id, r.instance_id)
                        .map(|_| Vec::new())
                        .map_err(|m| err(ErrorCode::ExecTrap, format!("vtab.destroy: {m}")))
                }
                other => Err(unknown(other)),
            }
        }
    }

    fn parse_op(s: &str) -> vt::ConstraintOp {
        use vt::ConstraintOp::*;
        match s {
            "gt" => Gt, "le" => Le, "lt" => Lt, "ge" => Ge, "ne" => Ne,
            "match" => Match, "like" => Like, "regexp" => Regexp, "glob" => Glob,
            "is-null" => IsNull, "is-not-null" => IsNotNull, "limit" => Limit,
            "offset" => Offset, "function" => Function, _ => Eq,
        }
    }

    // op_name kept for completeness of the surface (host-side decode aid).
    #[allow(dead_code)]
    fn _touch() {
        let _ = op_name as fn(vt::ConstraintOp) -> String;
    }

    export!(Provider);
}

// ===========================================================================
// SHAPE: vtab-mut  (world provider-vtab-mut) — read + mutating surface
// ===========================================================================
#[cfg(feature = "vtab-mut")]
mod shape {
    use super::envelope::*;
    provider_impl!("provider-vtab-mut");
    sqlvalue_conv!();
    use sqlite::extension::vtab_update as vu;

    impl Guest for Provider {
        fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
            common_methods!(method.as_str(), &payload);
            match method.as_str() {
                "vtab-update.update" => {
                    let r: VtabUpdateReq = decode(&payload)?;
                    match vu::update(r.vtab_id, r.instance_id, &args_to_wit(&r.args)) {
                        Ok(rowid) => encode(&rowid),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("vtab-update.update: {m}"))),
                    }
                }
                "vtab-update.begin" => txn(decode(&payload)?, vu::begin),
                "vtab-update.sync" => txn(decode(&payload)?, vu::sync),
                "vtab-update.commit" => txn(decode(&payload)?, vu::commit),
                "vtab-update.rollback" => txn(decode(&payload)?, vu::rollback),
                other => Err(unknown(other)),
            }
        }
    }

    fn txn(
        r: VtabInstanceReq,
        f: fn(u64, u64) -> Result<(), String>,
    ) -> Result<Vec<u8>, Error> {
        match f(r.vtab_id, r.instance_id) {
            Ok(()) => encode(&()),
            Err(m) => Err(err(ErrorCode::ExecTrap, format!("vtab-update txn: {m}"))),
        }
    }

    export!(Provider);
}

// ===========================================================================
// SHAPE: hooks  (world provider-hooks) — authorizer + hook callbacks
// ===========================================================================
#[cfg(feature = "hooks")]
mod shape {
    use super::envelope::*;
    provider_impl!("provider-hooks");
    sqlvalue_conv!();
    use sqlite::extension::authorizer as az;
    use sqlite::extension::commit_hook as ch;
    use sqlite::extension::types as t;
    use sqlite::extension::update_hook as uh;
    use sqlite::extension::wal_hook as wh;

    fn parse_action(s: &str) -> t::AuthAction {
        use t::AuthAction::*;
        match s {
            "create-index" => CreateIndex, "create-table" => CreateTable,
            "create-temp-index" => CreateTempIndex, "create-temp-table" => CreateTempTable,
            "create-temp-trigger" => CreateTempTrigger, "create-temp-view" => CreateTempView,
            "create-trigger" => CreateTrigger, "create-view" => CreateView,
            "delete" => Delete, "drop-index" => DropIndex, "drop-table" => DropTable,
            "drop-temp-index" => DropTempIndex, "drop-temp-table" => DropTempTable,
            "drop-temp-trigger" => DropTempTrigger, "drop-temp-view" => DropTempView,
            "drop-trigger" => DropTrigger, "drop-view" => DropView, "insert" => Insert,
            "pragma" => Pragma, "read" => Read, "select" => Select,
            "transaction" => Transaction, "update" => Update, "attach" => Attach,
            "detach" => Detach, "alter-table" => AlterTable, "reindex" => Reindex,
            "analyze" => Analyze, "create-vtable" => CreateVtable, "drop-vtable" => DropVtable,
            "function" => Function, "savepoint" => Savepoint, _ => Recursive,
        }
    }

    fn result_name(r: t::AuthResult) -> &'static str {
        match r {
            t::AuthResult::Ok => "ok",
            t::AuthResult::Deny => "deny",
            t::AuthResult::Ignore => "ignore",
        }
    }

    fn parse_update_op(s: &str) -> t::UpdateOperation {
        match s {
            "update" => t::UpdateOperation::Update,
            "delete" => t::UpdateOperation::Delete,
            _ => t::UpdateOperation::Insert,
        }
    }

    impl Guest for Provider {
        fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
            common_methods!(method.as_str(), &payload);
            match method.as_str() {
                "call" => {
                    let req: CallReq = decode(&payload)?;
                    match sqlite::extension::scalar_function::call(req.func_id, &args_to_wit(&req.args)) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("scalar call: {m}"))),
                    }
                }
                "authorizer.authorize" => {
                    let r: AuthorizeReq = decode(&payload)?;
                    let res = az::authorize(
                        parse_action(&r.action),
                        r.arg1.as_deref(),
                        r.arg2.as_deref(),
                        r.database.as_deref(),
                        r.trigger.as_deref(),
                    );
                    encode(&result_name(res).to_string())
                }
                "hook.update" => {
                    let r: UpdateHookReq = decode(&payload)?;
                    uh::on_update(parse_update_op(&r.operation), &r.database, &r.table, r.rowid);
                    encode(&())
                }
                "hook.commit" => {
                    // on-commit returns true to convert commit -> rollback.
                    encode(&ch::on_commit())
                }
                "hook.rollback" => {
                    ch::on_rollback();
                    encode(&())
                }
                "hook.wal" => {
                    let r: WalHookReq = decode(&payload)?;
                    let rc = wh::on_wal_hook(r.hook_id, &r.db_name, r.n_frames_in_wal);
                    encode(&rc)
                }
                other => Err(unknown(other)),
            }
        }
    }
    export!(Provider);
}

// ===========================================================================
// SHAPE: dotcmd  (world provider-dotcmd) — dot-command invoke + the
// cli-stdout / cli-stderr / cli-state host streams the provider EXPORTS to
// satisfy the extension's import and CAPTURE its streamed output.
// ===========================================================================
#[cfg(feature = "dotcmd")]
mod shape {
    use super::envelope::*;
    use std::cell::RefCell;
    provider_impl!("provider-dotcmd");
    sqlvalue_conv!();
    use sqlite::extension::dot_command as dc;

    thread_local! {
        static STDOUT: RefCell<String> = RefCell::new(String::new());
        static STDERR: RefCell<String> = RefCell::new(String::new());
    }

    // The provider IS the cli host for the plugged extension: it exports
    // cli-stdout / cli-stderr / cli-state. Writes are captured and folded
    // into the invoke response envelope (output-capture, not reentrancy).
    impl exports::sqlite::extension::cli_stdout::Guest for Provider {
        fn write(text: String) {
            STDOUT.with(|s| s.borrow_mut().push_str(&text));
        }
        fn flush() {}
        fn row_end() {
            STDOUT.with(|s| s.borrow_mut().push('\n'));
        }
    }

    impl exports::sqlite::extension::cli_stderr::Guest for Provider {
        fn write(text: String) {
            STDERR.with(|s| s.borrow_mut().push_str(&text));
        }
    }

    impl exports::sqlite::extension::cli_state::Guest for Provider {
        // The declarative bridge holds no live cli session; return
        // schema defaults. (A host-driven dotcmd that needs live state is
        // the reentrant tier.)
        fn get_text(_key: String) -> String { String::new() }
        fn get_int(_key: String) -> i64 { 0 }
        fn get_bool(_key: String) -> bool { false }
        fn get_real(_key: String) -> f64 { 0.0 }
        fn get_value(_key: String) -> sqlite::extension::types::SqlValue {
            sqlite::extension::types::SqlValue::Null
        }
        fn list_keys(_prefix: String) -> Vec<String> { Vec::new() }
    }

    impl Guest for Provider {
        fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
            common_methods!(method.as_str(), &payload);
            match method.as_str() {
                "call" => {
                    let req: CallReq = decode(&payload)?;
                    match sqlite::extension::scalar_function::call(req.func_id, &args_to_wit(&req.args)) {
                        Ok(v) => encode(&from_wit(v)),
                        Err(m) => Err(err(ErrorCode::ExecTrap, format!("scalar call: {m}"))),
                    }
                }
                "dotcmd.invoke" => {
                    let r: DotInvokeReq = decode(&payload)?;
                    STDOUT.with(|s| s.borrow_mut().clear());
                    STDERR.with(|s| s.borrow_mut().clear());
                    let ctx = dc::InvokeContext {
                        args: r.args,
                        interactive: r.interactive,
                        display_mode: r.display_mode,
                        bail_on_error: r.bail_on_error,
                    };
                    match dc::invoke(r.func_id, &ctx) {
                        Ok(res) => encode(&DotInvokeResp {
                            text: res.text,
                            ok: res.ok,
                            exit_code: res.exit_code,
                            stdout: STDOUT.with(|s| s.borrow().clone()),
                            stderr: STDERR.with(|s| s.borrow().clone()),
                        }),
                        Err(e) => Err(err(
                            ErrorCode::ExecTrap,
                            format!("dotcmd.invoke: [{}] {}", e.code, e.message),
                        )),
                    }
                }
                other => Err(unknown(other)),
            }
        }
    }
    export!(Provider);
}
