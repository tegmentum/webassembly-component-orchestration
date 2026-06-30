//! `dotret` — a minimal declarative DOT-COMMAND extension (world
//! `dotcmd-aware`) that returns its output via `invoke-result.text`
//! rather than streaming through the `cli-stdout` host import. Never
//! calling cli-stdout tree-shakes that import, so the composed provider
//! has no cyclic dependency.
//!
//! `.echo <args>` -> returns "echo: <args>" in invoke-result.text.

mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "dotcmd-aware",
        generate_all,
    });
}

use bindings::exports::sqlite::extension::dot_command::{
    Guest as DotGuest, InvokeContext, InvokeResult,
};
use bindings::exports::sqlite::extension::metadata::{
    DotCommandSpec, Guest as MetaGuest, Manifest,
};
use bindings::exports::sqlite::extension::scalar_function::Guest as ScalarGuest;
use bindings::sqlite::extension::types::{SqliteError, SqlValue};

const FID_ECHO: u64 = 1;

struct Ext;

impl MetaGuest for Ext {
    fn describe() -> Manifest {
        Manifest {
            name: "dotret".to_string(),
            version: "0.1.0".to_string(),
            scalar_functions: vec![],
            aggregate_functions: vec![],
            collations: vec![],
            vtabs: vec![],
            dot_commands: vec![DotCommandSpec {
                id: FID_ECHO,
                name: "echo".to_string(),
                version: "0.1.0".to_string(),
                summary: "Echo the argument string".to_string(),
                usage: "echo <text>".to_string(),
                help: "Returns 'echo: <text>'.".to_string(),
                examples: vec![],
                requires_write: false,
                no_args: false,
            }],
            has_authorizer: false,
            has_update_hook: false,
            has_commit_hook: false,
            has_wal_hook: false,
            wal_hook_id: 0,
            declared_capabilities: vec![],
            optional_capabilities: vec![],
            preferred_prefix: Some("dotret".to_string()),
            prefix_expansion: Some("com.tegmentum.test.dotret".to_string()),
            typed_values: vec![],
        }
    }
}

impl ScalarGuest for Ext {
    fn call(_func_id: u64, _args: Vec<SqlValue>) -> Result<SqlValue, String> {
        Err("dotret exports only a dot-command".to_string())
    }
}

impl DotGuest for Ext {
    fn invoke(func_id: u64, ctx: InvokeContext) -> Result<InvokeResult, SqliteError> {
        if func_id != FID_ECHO {
            return Err(SqliteError {
                code: 1,
                extended_code: 1,
                message: format!("dotret: unknown func id {func_id}"),
            });
        }
        Ok(InvokeResult {
            // Return output directly; do NOT call cli-stdout (no cycle).
            text: format!("echo: {}", ctx.args),
            state_deltas: vec![],
            ok: true,
            exit_code: 0,
        })
    }
}

bindings::export!(Ext with_types_in bindings);
