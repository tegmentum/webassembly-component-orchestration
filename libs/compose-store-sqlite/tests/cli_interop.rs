//! Verifies cross-tool interop with sqlite-wasm-cli's
//! orchestration module. Both sides use the same
//! `_compose_plans` schema; rows written by either should be
//! readable by either. This test only exercises the read path
//! against a pre-populated db (the cli-side write path lives
//! in the sqlite-wasm tree and runs there).

use compose_store_sqlite::{SqliteComposeStore, FORMAT_V1};
use rusqlite::{params, Connection};

const CLI_SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS _compose_plans (
    name        TEXT PRIMARY KEY,
    version     TEXT NOT NULL,
    root        TEXT NOT NULL,
    digest_hex  TEXT NOT NULL,
    format      TEXT NOT NULL,
    body        BLOB NOT NULL,
    saved_at    INTEGER NOT NULL
);
";

#[test]
fn reads_a_cli_written_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("user.sqlite");
    // Simulate the cli writing a row directly (using rusqlite
    // because the cli's sqlite_wasm_core wrapper would speak the
    // same SQL).
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(CLI_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO _compose_plans \
                (name, version, root, digest_hex, format, body, saved_at) \
             VALUES (?1, '', '', ?2, ?3, ?4, 1700000000)",
            params![
                "from-cli",
                "ae48a365f64edb88acd908b821b232e83fa58fcbbc045d956395e79bb2ae8754",
                FORMAT_V1,
                b"{\"version\":\"1\",\"root\":\"demo\"}" as &[u8],
            ],
        )
        .unwrap();
    }
    // Now read it back via the orchestrator-side store.
    let s = SqliteComposeStore::open(&path).unwrap();
    let row = s.get("from-cli").unwrap().expect("row");
    assert_eq!(row.name, "from-cli");
    assert_eq!(row.format, FORMAT_V1);
    assert_eq!(row.saved_at, 1_700_000_000);
    assert_eq!(row.body, b"{\"version\":\"1\",\"root\":\"demo\"}");
}
