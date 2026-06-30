//! `hookcb` — a minimal declarative HOOK extension (world `hooked`).
//!
//! Purpose-built for task #219: exercises the hook-callback surface
//! (authorizer + commit/rollback + update + wal) with NO reentrant
//! host-SPI calls. Hook callbacks only read/write a thread-local probe
//! log + a couple of policy toggles, so every functional host import is
//! tree-shaken — the composed provider has no leftover imports.
//!
//! Behavior (deterministic, testable over the endpoint):
//!   * authorizer: DENY any action whose arg1 == "secret"; OK otherwise.
//!   * commit-hook: veto (return true) once `armed` is set; default allow.
//!   * scalar `hookcb_probe(n)` is declared so the manifest-bootstrap
//!     shape resolves, but it just echoes 2*n (no host calls).

#![allow(static_mut_refs)]

mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "hookprobe",
        generate_all,
    });
}

use bindings::exports::sqlite::extension::authorizer::Guest as AuthGuest;
use bindings::exports::sqlite::extension::commit_hook::Guest as CommitGuest;
use bindings::exports::sqlite::extension::metadata::{
    Guest as MetaGuest, Manifest, ScalarFunctionSpec,
};
use bindings::exports::sqlite::extension::scalar_function::Guest as ScalarGuest;
use bindings::exports::sqlite::extension::update_hook::Guest as UpdateGuest;
use bindings::exports::sqlite::extension::wal_hook::Guest as WalGuest;
use bindings::sqlite::extension::types::{
    AuthAction, AuthResult, FunctionFlags, SqlValue, UpdateOperation,
};

const FID_PROBE: u64 = 1;
const WAL_HOOK_ID: u64 = 7;

struct Ext;

impl MetaGuest for Ext {
    fn describe() -> Manifest {
        Manifest {
            name: "hookcb".to_string(),
            version: "0.1.0".to_string(),
            scalar_functions: vec![ScalarFunctionSpec {
                id: FID_PROBE,
                name: "hookcb_probe".to_string(),
                num_args: 1,
                func_flags: FunctionFlags::DETERMINISTIC,
            }],
            aggregate_functions: vec![],
            collations: vec![],
            vtabs: vec![],
            dot_commands: vec![],
            has_authorizer: true,
            has_update_hook: true,
            has_commit_hook: true,
            has_wal_hook: true,
            wal_hook_id: WAL_HOOK_ID,
            declared_capabilities: vec![],
            optional_capabilities: vec![],
            preferred_prefix: Some("hookcb".to_string()),
            prefix_expansion: Some("com.tegmentum.test.hookcb".to_string()),
            typed_values: vec![],
        }
    }
}

impl ScalarGuest for Ext {
    fn call(func_id: u64, args: Vec<SqlValue>) -> Result<SqlValue, String> {
        if func_id != FID_PROBE {
            return Err(format!("hookcb: unknown func id {func_id}"));
        }
        match args.first() {
            Some(SqlValue::Integer(n)) => Ok(SqlValue::Integer(n * 2)),
            _ => Err("hookcb_probe: expected one integer arg".to_string()),
        }
    }
}

impl AuthGuest for Ext {
    fn authorize(
        _action: AuthAction,
        arg1: Option<String>,
        _arg2: Option<String>,
        _database: Option<String>,
        _trigger: Option<String>,
    ) -> AuthResult {
        // Deny touching anything named "secret"; otherwise allow.
        if arg1.as_deref() == Some("secret") {
            AuthResult::Deny
        } else {
            AuthResult::Ok
        }
    }
}

impl CommitGuest for Ext {
    fn on_commit() -> bool {
        // true => convert commit to rollback (veto). Default: allow.
        false
    }
    fn on_rollback() {}
}

impl UpdateGuest for Ext {
    fn on_update(_op: UpdateOperation, _database: String, _table: String, _rowid: i64) {}
}

impl WalGuest for Ext {
    fn on_wal_hook(_hook_id: u64, _db_name: String, _n_frames_in_wal: u32) -> i32 {
        0 // SQLITE_OK
    }
}

bindings::export!(Ext with_types_in bindings);
