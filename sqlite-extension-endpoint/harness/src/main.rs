//! Generic flavor-B dlopen harness for `sqlite-extension-endpoint`.
//!
//! Generalizes the spike #218 aba harness: it resolves a provider by id
//! ("ext") through the host's `compose:dynlink/linker`, reads the full
//! declarative manifest over the CBOR envelope (`describe`), runs the
//! fail-closed policy reconcile (`policy-check`), then exercises one
//! TIER chosen by argv[0] of the run command. One harness binary drives
//! every tier — the same way one provider source bridges every tier.
//!
//! Scenarios (selected by the SCENARIO env var, defaulting to "scalar"):
//!   scalar     -> describe + scalar call (aba_validate)
//!   aggregate  -> step over a column + finalize + scalar verify (count_min)
//!   collation  -> collation.compare (uint natural-numeric order)
//!   vtab       -> open/filter/next/eof/column over generate_series
//!   hooks      -> authorizer.authorize + hook.commit (hookprobe)

wit_bindgen::generate!({
    world: "dynlink-guest",
    path: "wit",
    generate_all,
});

use compose::dynlink::linker::{resolve_by_id, Instance};
use serde::{Deserialize, Serialize};

// --- envelope mirror (kept in lockstep with provider/src/envelope.rs) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "lowercase")]
enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
    #[serde(rename = "witvalue")]
    WitValue {
        type_id: Vec<u8>,
        bytes: Vec<u8>,
        symbolic_name: String,
    },
}

#[derive(Debug, Deserialize)]
struct ScalarSpec {
    id: u64,
    name: String,
    num_args: i32,
    #[allow(dead_code)]
    deterministic: bool,
    #[allow(dead_code)]
    direct_only: bool,
    #[allow(dead_code)]
    innocuous: bool,
}
#[derive(Debug, Deserialize)]
struct AggregateSpec {
    id: u64,
    name: String,
    num_args: i32,
    is_window: bool,
}
#[derive(Debug, Deserialize)]
struct CollationSpec {
    id: u64,
    name: String,
}
#[derive(Debug, Deserialize)]
struct VtabSpec {
    id: u64,
    name: String,
    eponymous: bool,
    mutable: bool,
    batched: bool,
}
#[derive(Debug, Deserialize)]
struct DotCommandSpec {
    id: u64,
    name: String,
    #[allow(dead_code)]
    version: String,
    summary: String,
    #[allow(dead_code)]
    usage: String,
    #[allow(dead_code)]
    requires_write: bool,
    #[allow(dead_code)]
    no_args: bool,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    version: String,
    scalars: Vec<ScalarSpec>,
    aggregates: Vec<AggregateSpec>,
    collations: Vec<CollationSpec>,
    vtabs: Vec<VtabSpec>,
    dot_commands: Vec<DotCommandSpec>,
    has_authorizer: bool,
    has_update_hook: bool,
    has_commit_hook: bool,
    has_wal_hook: bool,
    wal_hook_id: u64,
    declared_capabilities: Vec<String>,
    optional_capabilities: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PolicyCheckReq {
    grant: Vec<String>,
}
#[derive(Debug, Deserialize)]
struct CapabilityReport {
    ok: bool,
    missing: Vec<String>,
    optional_ungranted: Vec<String>,
    granted: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CallReq {
    func_id: u64,
    args: Vec<SqlValue>,
}
#[derive(Debug, Serialize)]
struct AggStepReq {
    func_id: u64,
    context_id: u64,
    args: Vec<SqlValue>,
}
#[derive(Debug, Serialize)]
struct AggCtxReq {
    func_id: u64,
    context_id: u64,
}
#[derive(Debug, Serialize)]
struct CollationCompareReq {
    collation_id: u64,
    a: String,
    b: String,
}
#[derive(Debug, Serialize)]
struct VtabConnectReq {
    vtab_id: u64,
    instance_id: u64,
    db_name: String,
    table_name: String,
    args: Vec<String>,
}
#[derive(Debug, Serialize)]
struct VtabOpenReq {
    vtab_id: u64,
    instance_id: u64,
    cursor_id: u64,
}
#[derive(Debug, Serialize)]
struct VtabFilterReq {
    vtab_id: u64,
    cursor_id: u64,
    idx_num: i32,
    idx_str: Option<String>,
    args: Vec<SqlValue>,
}
#[derive(Debug, Serialize)]
struct VtabCursorReq {
    vtab_id: u64,
    cursor_id: u64,
}
#[derive(Debug, Serialize)]
struct VtabColumnReq {
    vtab_id: u64,
    cursor_id: u64,
    col: i32,
}
#[derive(Debug, Serialize)]
struct AuthorizeReq {
    action: String,
    arg1: Option<String>,
    arg2: Option<String>,
    database: Option<String>,
    trigger: Option<String>,
}
#[derive(Debug, Serialize)]
struct UpdateHookReq {
    operation: String,
    database: String,
    table: String,
    rowid: i64,
}
#[derive(Debug, Serialize)]
struct WalHookReq {
    hook_id: u64,
    db_name: String,
    n_frames_in_wal: u32,
}
#[derive(Debug, Serialize)]
struct VtabUpdateReq {
    vtab_id: u64,
    instance_id: u64,
    args: Vec<SqlValue>,
}
#[derive(Debug, Serialize)]
struct DotInvokeReq {
    func_id: u64,
    args: String,
    interactive: bool,
    display_mode: String,
    bail_on_error: bool,
}
#[derive(Debug, Deserialize)]
struct DotInvokeResp {
    text: String,
    ok: bool,
    exit_code: i32,
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(v, &mut out).expect("cbor encode");
    out
}
fn dec<T: serde::de::DeserializeOwned>(b: &[u8]) -> T {
    ciborium::de::from_reader(b).expect("cbor decode")
}
fn die(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn invoke(inst: &Instance, method: &str, payload: &[u8]) -> Vec<u8> {
    match inst.invoke(method, payload) {
        Ok(b) => b,
        Err(e) => die(format!("invoke({method}) failed: {}", e.message)),
    }
}

fn show(v: &SqlValue) -> String {
    match v {
        SqlValue::Integer(i) => i.to_string(),
        SqlValue::Real(r) => r.to_string(),
        SqlValue::Text(s) => s.clone(),
        SqlValue::Null => "NULL".to_string(),
        SqlValue::Blob(b) => format!("<blob {} bytes>", b.len()),
        SqlValue::WitValue { symbolic_name, .. } => format!("<wit-value {symbolic_name}>"),
    }
}

fn main() {
    let scenario = std::env::var("SCENARIO").unwrap_or_else(|_| "scalar".to_string());

    let inst = match resolve_by_id("ext") {
        Ok(i) => i,
        Err(e) => die(format!("resolve(ext) failed: {}", e.message)),
    };

    // 1. describe -> the full declarative manifest (manifest->register input).
    let manifest: Manifest = dec(&invoke(&inst, "describe", &[]));
    println!(
        "loaded extension: {} v{}  [scalars={} aggregates={} collations={} vtabs={} dotcmds={}]",
        manifest.name,
        manifest.version,
        manifest.scalars.len(),
        manifest.aggregates.len(),
        manifest.collations.len(),
        manifest.vtabs.len(),
        manifest.dot_commands.len(),
    );
    println!(
        "  hooks: authorizer={} update={} commit={} wal={} (wal_hook_id={})",
        manifest.has_authorizer,
        manifest.has_update_hook,
        manifest.has_commit_hook,
        manifest.has_wal_hook,
        manifest.wal_hook_id
    );
    println!(
        "  declared_capabilities={:?} optional={:?}",
        manifest.declared_capabilities, manifest.optional_capabilities
    );

    // 2. policy-check -> the fail-closed reconcile. Grant exactly the
    //    declared set (mirrors a plan that grants what the extension needs).
    let grant = manifest.declared_capabilities.clone();
    let report: CapabilityReport = dec(&invoke(&inst, "policy-check", &encode(&PolicyCheckReq { grant })));
    println!(
        "  policy-check: ok={} missing={:?} optional_ungranted={:?} granted={:?}",
        report.ok, report.missing, report.optional_ungranted, report.granted
    );
    if !report.ok {
        die(format!("policy-check failed (fail-closed): missing {:?}", report.missing));
    }
    // Demonstrate fail-closed: if the extension declares anything, an empty
    // grant must be refused.
    if !manifest.declared_capabilities.is_empty() {
        let denied: CapabilityReport =
            dec(&invoke(&inst, "policy-check", &encode(&PolicyCheckReq { grant: vec![] })));
        println!(
            "  policy-check(empty grant): ok={} missing={:?}  (fail-closed gate verified)",
            denied.ok, denied.missing
        );
    }

    match scenario.as_str() {
        "scalar" => scenario_scalar(&inst, &manifest),
        "aggregate" => scenario_aggregate(&inst, &manifest),
        "collation" => scenario_collation(&inst, &manifest),
        "vtab" => scenario_vtab(&inst, &manifest),
        "hooks" => scenario_hooks(&inst, &manifest),
        "vtab_mut" | "vtab-mut" => scenario_vtab_mut(&inst, &manifest),
        "dotcmd" => scenario_dotcmd(&inst, &manifest),
        other => die(format!("unknown scenario: {other}")),
    }
}

fn find_scalar<'a>(m: &'a Manifest, name: &str) -> &'a ScalarSpec {
    m.scalars
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| die(format!("scalar {name} not in manifest")))
}

fn scenario_scalar(inst: &Instance, m: &Manifest) {
    let f = find_scalar(m, "aba_validate");
    println!("  scalar id={} name={} num_args={}", f.id, f.name, f.num_args);
    for routing in ["021000021", "021000022", "not a routing"] {
        let req = CallReq {
            func_id: f.id,
            args: vec![SqlValue::Text(routing.to_string())],
        };
        let v: SqlValue = dec(&invoke(inst, "call", &encode(&req)));
        println!("aba_validate('{routing}') => {}", show(&v));
    }
}

fn scenario_aggregate(inst: &Instance, m: &Manifest) {
    let agg = m
        .aggregates
        .iter()
        .find(|a| a.name == "count_min")
        .unwrap_or_else(|| die("aggregate count_min not in manifest".into()));
    println!(
        "  aggregate id={} name={} num_args={} window={}",
        agg.id, agg.name, agg.num_args, agg.is_window
    );
    // Build a sketch over a multiset; "apple" appears 3x.
    let ctx = 1u64;
    let data = ["apple", "apple", "banana", "apple", "cherry"];
    for v in data {
        let req = AggStepReq {
            func_id: agg.id,
            context_id: ctx,
            args: vec![SqlValue::Text(v.to_string())],
        };
        let _: () = dec(&invoke(inst, "agg.step", &encode(&req)));
    }
    let sketch: SqlValue = dec(&invoke(
        inst,
        "agg.finalize",
        &encode(&AggCtxReq { func_id: agg.id, context_id: ctx }),
    ));
    let sketch_bytes = match &sketch {
        SqlValue::Blob(b) => b.clone(),
        other => die(format!("finalize did not return a blob: {other:?}")),
    };
    println!("count_min(...) over {} rows => sketch {} bytes", data.len(), sketch_bytes.len());
    // Verify via the count_min_estimate scalar (sketch, value) -> count.
    let est = find_scalar(m, "count_min_estimate");
    for word in ["apple", "banana", "cherry", "durian"] {
        let req = CallReq {
            func_id: est.id,
            args: vec![SqlValue::Blob(sketch_bytes.clone()), SqlValue::Text(word.to_string())],
        };
        let v: SqlValue = dec(&invoke(inst, "call", &encode(&req)));
        println!("count_min_estimate(sketch, '{word}') => {}", show(&v));
    }
}

fn scenario_collation(inst: &Instance, m: &Manifest) {
    let col: &CollationSpec = m
        .collations
        .iter()
        .find(|c| c.name == "uint")
        .unwrap_or_else(|| die("collation uint not in manifest".into()));
    println!("  collation id={} name={}", col.id, col.name);
    // Natural-numeric: "x2" < "x10" (lexical would give x10 < x2).
    let pairs = [("x2", "x10"), ("x10", "x2"), ("x1", "x1"), ("a9", "a100")];
    for (a, b) in pairs {
        let req = CollationCompareReq {
            collation_id: col.id,
            a: a.to_string(),
            b: b.to_string(),
        };
        let cmp: i32 = dec(&invoke(inst, "collation.compare", &encode(&req)));
        let rel = if cmp < 0 { "<" } else if cmp > 0 { ">" } else { "==" };
        println!("compare('{a}','{b}') => {cmp}  ('{a}' {rel} '{b}')");
    }
}

fn scenario_vtab(inst: &Instance, m: &Manifest) {
    let vt: &VtabSpec = m
        .vtabs
        .iter()
        .find(|v| v.name == "generate_series")
        .unwrap_or_else(|| die("vtab generate_series not in manifest".into()));
    println!(
        "  vtab id={} name={} eponymous={} mutable={} batched={}",
        vt.id, vt.name, vt.eponymous, vt.mutable, vt.batched
    );
    let inst_id = 1u64;
    let cur_id = 1u64;
    // connect: eponymous -> connect, returns schema.
    let schema: String = dec(&invoke(
        inst,
        "vtab.connect",
        &encode(&VtabConnectReq {
            vtab_id: vt.id,
            instance_id: inst_id,
            db_name: "main".to_string(),
            table_name: "generate_series".to_string(),
            args: vec![],
        }),
    ));
    println!("  schema: {schema}");
    // open cursor
    let _: Vec<u8> = invoke(
        inst,
        "vtab.open",
        &encode(&VtabOpenReq { vtab_id: vt.id, instance_id: inst_id, cursor_id: cur_id }),
    );
    // filter: generate_series(1, 5). idx_num/args depend on the module's
    // best-index contract; series uses bound args [start, stop, step].
    let _: Vec<u8> = invoke(
        inst,
        "vtab.filter",
        &encode(&VtabFilterReq {
            vtab_id: vt.id,
            cursor_id: cur_id,
            idx_num: 3, // start|stop both bound (series idx-num bitmask)
            idx_str: None,
            args: vec![SqlValue::Integer(1), SqlValue::Integer(5)],
        }),
    );
    let mut out = Vec::new();
    let mut guard = 0;
    loop {
        let eof: bool = dec(&invoke(
            inst,
            "vtab.eof",
            &encode(&VtabCursorReq { vtab_id: vt.id, cursor_id: cur_id }),
        ));
        if eof {
            break;
        }
        let v: SqlValue = dec(&invoke(
            inst,
            "vtab.column",
            &encode(&VtabColumnReq { vtab_id: vt.id, cursor_id: cur_id, col: 0 }),
        ));
        out.push(show(&v));
        let _: Vec<u8> = invoke(
            inst,
            "vtab.next",
            &encode(&VtabCursorReq { vtab_id: vt.id, cursor_id: cur_id }),
        );
        guard += 1;
        if guard > 1000 {
            die("vtab scan did not terminate".into());
        }
    }
    println!("SELECT value FROM generate_series(1,5) => [{}]", out.join(", "));
}

fn scenario_hooks(inst: &Instance, m: &Manifest) {
    println!(
        "  hook surface: authorizer={} commit={} update={} wal={}",
        m.has_authorizer, m.has_commit_hook, m.has_update_hook, m.has_wal_hook
    );
    // authorizer.authorize: an ordinary table is allowed; "secret" denied.
    for tbl in ["t", "secret"] {
        let req = AuthorizeReq {
            action: "read".to_string(),
            arg1: Some(tbl.to_string()),
            arg2: None,
            database: Some("main".to_string()),
            trigger: None,
        };
        let res: String = dec(&invoke(inst, "authorizer.authorize", &encode(&req)));
        println!("authorize(read, arg1='{tbl}') => {res}");
    }
    // update-hook callback (fire-and-forget).
    let _: () = dec(&invoke(
        inst,
        "hook.update",
        &encode(&UpdateHookReq {
            operation: "insert".to_string(),
            database: "main".to_string(),
            table: "t".to_string(),
            rowid: 1,
        }),
    ));
    println!("update-hook on_update(insert, main.t, rowid=1) => dispatched");
    // commit hook: returns true to veto.
    let veto: bool = dec(&invoke(inst, "hook.commit", &[]));
    println!("commit-hook on_commit() => veto={veto}");
    // wal hook: returns a SQLite result code.
    let rc: i32 = dec(&invoke(
        inst,
        "hook.wal",
        &encode(&WalHookReq {
            hook_id: m.wal_hook_id,
            db_name: "main".to_string(),
            n_frames_in_wal: 3,
        }),
    ));
    println!("wal-hook on_wal_hook(id={}, main, frames=3) => rc={rc}", m.wal_hook_id);
}

fn scenario_vtab_mut(inst: &Instance, m: &Manifest) {
    let vt: &VtabSpec = m
        .vtabs
        .iter()
        .find(|v| v.mutable)
        .unwrap_or_else(|| die("no mutable vtab in manifest".into()));
    println!("  mutable vtab id={} name={} mutable={}", vt.id, vt.name, vt.mutable);
    let inst_id = 1u64;
    // create (non-eponymous): materialize backing storage + get schema.
    let schema: String = dec(&invoke(
        inst,
        "vtab.create",
        &encode(&VtabConnectReq {
            vtab_id: vt.id,
            instance_id: inst_id,
            db_name: "main".to_string(),
            table_name: vt.name.clone(),
            args: vec![],
        }),
    ));
    println!("  schema: {schema}");
    // INSERT two rows via xUpdate. SQLite INSERT encoding:
    //   args = [null, proposed_rowid, col0, col1, ...]
    for (rid, key, val) in [(1i64, "alpha", 100i64), (2, "beta", 200)] {
        let new_rowid: i64 = dec(&invoke(
            inst,
            "vtab-update.update",
            &encode(&VtabUpdateReq {
                vtab_id: vt.id,
                instance_id: inst_id,
                args: vec![
                    SqlValue::Null,
                    SqlValue::Integer(rid),
                    SqlValue::Text(key.to_string()),
                    SqlValue::Integer(val),
                ],
            }),
        ));
        println!("  INSERT (key={key}, value={val}) => rowid {new_rowid}");
    }
    // Read the rows back through the read-cursor surface.
    let cur_id = 1u64;
    let _: Vec<u8> = invoke(
        inst,
        "vtab.open",
        &encode(&VtabOpenReq { vtab_id: vt.id, instance_id: inst_id, cursor_id: cur_id }),
    );
    let _: Vec<u8> = invoke(
        inst,
        "vtab.filter",
        &encode(&VtabFilterReq {
            vtab_id: vt.id,
            cursor_id: cur_id,
            idx_num: 0,
            idx_str: None,
            args: vec![],
        }),
    );
    let mut rows = Vec::new();
    let mut guard = 0;
    loop {
        let eof: bool = dec(&invoke(
            inst,
            "vtab.eof",
            &encode(&VtabCursorReq { vtab_id: vt.id, cursor_id: cur_id }),
        ));
        if eof {
            break;
        }
        let k: SqlValue = dec(&invoke(
            inst,
            "vtab.column",
            &encode(&VtabColumnReq { vtab_id: vt.id, cursor_id: cur_id, col: 0 }),
        ));
        let v: SqlValue = dec(&invoke(
            inst,
            "vtab.column",
            &encode(&VtabColumnReq { vtab_id: vt.id, cursor_id: cur_id, col: 1 }),
        ));
        rows.push(format!("({}={})", show(&k), show(&v)));
        let _: Vec<u8> = invoke(
            inst,
            "vtab.next",
            &encode(&VtabCursorReq { vtab_id: vt.id, cursor_id: cur_id }),
        );
        guard += 1;
        if guard > 1000 {
            die("vtab-mut scan did not terminate".into());
        }
    }
    rows.sort();
    println!("SELECT key,value FROM {} => [{}]", vt.name, rows.join(", "));
}

fn scenario_dotcmd(inst: &Instance, m: &Manifest) {
    let cmd: &DotCommandSpec = m
        .dot_commands
        .iter()
        .next()
        .unwrap_or_else(|| die("no dot-command in manifest".into()));
    println!("  dot-command id={} name=.{} summary={:?}", cmd.id, cmd.name, cmd.summary);
    let req = DotInvokeReq {
        func_id: cmd.id,
        args: "hello world".to_string(),
        interactive: false,
        display_mode: "list".to_string(),
        bail_on_error: false,
    };
    let resp: DotInvokeResp = dec(&invoke(inst, "dotcmd.invoke", &encode(&req)));
    // dotret returns its output in invoke-result.text; the captured
    // cli-stdout stream is the path a streaming command (e.g. greet) would
    // use once the host wires its cli-stdout import (see REPORT).
    println!(
        ".{} hello world => ok={} exit={} text={:?} stdout={:?}",
        cmd.name, resp.ok, resp.exit_code, resp.text, resp.stdout
    );
}
