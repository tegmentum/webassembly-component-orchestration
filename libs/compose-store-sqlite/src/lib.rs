//! SQLite-backed storage for `compose-core` PlanV1 orchestration
//! definitions.
//!
//! # Purpose
//!
//! `compose-core` defines the canonical `PlanV1` shape but doesn't
//! prescribe how it's persisted. Different deployments want
//! different storage layers — filesystem, content-addressed
//! blob store, distributed KV. This crate ships the SQLite
//! variant: same wire shape (CBOR via `compose_core::plan::serialize`)
//! stored in a `_compose_plans` table.
//!
//! Primary consumer: `sqlite-wasm-cli`'s `.compose save|load|list`
//! dot-commands, where the orchestration plans live alongside the
//! user's data + capability grants in the same SQLite db. The cli
//! wraps `SqliteComposeStore` in a trait adapter (its own
//! `OrchestrationStore` shape) so the boundary stays clean.
//!
//! # Schema
//!
//! ```sql
//! CREATE TABLE _compose_plans (
//!     name        TEXT PRIMARY KEY,    -- caller-chosen display name
//!     version     TEXT NOT NULL,       -- copied from plan.version
//!     root        TEXT NOT NULL,       -- copied from plan.root (display)
//!     digest_hex  TEXT NOT NULL,       -- sha-256 of canonical CBOR
//!     format      TEXT NOT NULL,       -- 'compose-core-plan-v1-cbor'
//!     body        BLOB NOT NULL,       -- serialized PlanV1
//!     saved_at    INTEGER NOT NULL     -- unix seconds
//! );
//! CREATE INDEX _compose_plans_digest ON _compose_plans(digest_hex);
//! ```
//!
//! Schema versioning lives in `_compose_plans_meta`; a v2 migration
//! follows the cas-cache pattern (idempotent ALTER + meta row bump).

use anyhow::{anyhow, Result};
use compose_core::{
    plan::{compute_plan_digest, deserialize, serialize},
    types::PlanV1,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::time::{SystemTime, UNIX_EPOCH};

/// Format tag for the body BLOB. Pinned so a future v2 plan
/// format can coexist on the same row.
pub const FORMAT_V1: &str = "compose-core-plan-v1-cbor";

const SCHEMA_VERSION: &str = "1";

const SCHEMA_DDL: &str = "\
BEGIN;
CREATE TABLE IF NOT EXISTS _compose_plans (
    name        TEXT PRIMARY KEY,
    version     TEXT NOT NULL,
    root        TEXT NOT NULL,
    digest_hex  TEXT NOT NULL,
    format      TEXT NOT NULL,
    body        BLOB NOT NULL,
    saved_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS _compose_plans_digest ON _compose_plans(digest_hex);
CREATE TABLE IF NOT EXISTS _compose_plans_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO _compose_plans_meta(key, value) VALUES ('schema_version', '1');
COMMIT;
";

/// One row in `_compose_plans`. Returned by `get` / `list_full`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPlan {
    pub name: String,
    pub version: String,
    pub root: String,
    pub digest_hex: String,
    pub format: String,
    pub body: Vec<u8>,
    pub saved_at: i64,
}

/// SQLite-backed store for `PlanV1` orchestration definitions.
///
/// The wrapper owns its `rusqlite::Connection` and ensures the
/// `_compose_plans` schema on every method that needs it. The
/// connection is whatever the caller hands in — file-backed,
/// in-memory, or shared with another subsystem.
pub struct SqliteComposeStore {
    conn: Connection,
}

impl SqliteComposeStore {
    /// Open an external store at `path` (file or `:memory:`).
    /// Calls `Connection::open` + installs the schema.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        let mut store = Self { conn };
        store.ensure_schema()?;
        Ok(store)
    }

    /// Build a store over a pre-opened connection. Useful when
    /// the caller (e.g. sqlite-wasm-cli) already owns the user
    /// db connection and wants to layer `_compose_plans` on top
    /// without opening a second handle.
    pub fn from_connection(conn: Connection) -> Result<Self> {
        let mut store = Self { conn };
        store.ensure_schema()?;
        Ok(store)
    }

    /// Idempotent. Calls execute_batch on `SCHEMA_DDL`; safe to
    /// re-run.
    pub fn ensure_schema(&mut self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_DDL)?;
        Ok(())
    }

    /// Insert or replace by name. Computes the plan digest via
    /// `compose_core::plan::compute_plan_digest` so future
    /// digest-keyed lookups match what the orchestrator computes.
    pub fn put(&mut self, name: &str, plan: &PlanV1) -> Result<()> {
        let body = serialize(plan).map_err(|e| anyhow!("serialize plan: {e}"))?;
        let digest = compute_plan_digest(plan).map_err(|e| anyhow!("compute plan digest: {e}"))?;
        let digest_hex = hex::encode(&digest);
        let saved_at = unix_now();
        self.conn.execute(
            "INSERT OR REPLACE INTO _compose_plans \
                (name, version, root, digest_hex, format, body, saved_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                name,
                &plan.version,
                &plan.root,
                digest_hex,
                FORMAT_V1,
                body,
                saved_at,
            ],
        )?;
        Ok(())
    }

    /// Fetch a stored plan by name. Returns `None` if no row.
    pub fn get(&self, name: &str) -> Result<Option<StoredPlan>> {
        let row = self
            .conn
            .query_row(
                "SELECT name, version, root, digest_hex, format, body, saved_at \
                 FROM _compose_plans WHERE name = ?1",
                params![name],
                |r| {
                    Ok(StoredPlan {
                        name: r.get(0)?,
                        version: r.get(1)?,
                        root: r.get(2)?,
                        digest_hex: r.get(3)?,
                        format: r.get(4)?,
                        body: r.get(5)?,
                        saved_at: r.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Fetch + deserialize a stored plan in one step. Errors if
    /// the row's format isn't recognized.
    pub fn get_plan(&self, name: &str) -> Result<Option<PlanV1>> {
        let Some(row) = self.get(name)? else {
            return Ok(None);
        };
        if row.format != FORMAT_V1 {
            return Err(anyhow!(
                "stored plan '{name}' has unknown format {}",
                row.format
            ));
        }
        let plan = deserialize(&row.body).map_err(|e| anyhow!("deserialize plan: {e}"))?;
        Ok(Some(plan))
    }

    /// All stored plans, ordered by name. Body is included; use
    /// `list_names` if only the display info is wanted.
    pub fn list_full(&self) -> Result<Vec<StoredPlan>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, version, root, digest_hex, format, body, saved_at \
             FROM _compose_plans ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(StoredPlan {
                name: r.get(0)?,
                version: r.get(1)?,
                root: r.get(2)?,
                digest_hex: r.get(3)?,
                format: r.get(4)?,
                body: r.get(5)?,
                saved_at: r.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Just the names. Cheap. Useful when the caller renders a
    /// dot-command listing.
    pub fn list_names(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM _compose_plans ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Drop a row. Returns true iff a row was actually removed.
    pub fn delete(&mut self, name: &str) -> Result<bool> {
        let changed = self
            .conn
            .execute("DELETE FROM _compose_plans WHERE name = ?1", params![name])?;
        Ok(changed > 0)
    }

    /// Total row count. Useful for `.compose stats` style display
    /// in callers.
    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM _compose_plans", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// Look up by digest hex (alternate identity, useful when
    /// the caller already knows the canonical plan digest from
    /// the orchestrator's own logs).
    pub fn get_by_digest(&self, digest_hex: &str) -> Result<Option<StoredPlan>> {
        let row = self
            .conn
            .query_row(
                "SELECT name, version, root, digest_hex, format, body, saved_at \
                 FROM _compose_plans WHERE digest_hex = ?1 LIMIT 1",
                params![digest_hex],
                |r| {
                    Ok(StoredPlan {
                        name: r.get(0)?,
                        version: r.get(1)?,
                        root: r.get(2)?,
                        digest_hex: r.get(3)?,
                        format: r.get(4)?,
                        body: r.get(5)?,
                        saved_at: r.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Borrow the inner connection. Useful for callers that
    /// want to run their own SQL (e.g. cli displaying plans
    /// joined against capability_grants).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Re-export the schema version so callers writing migrations
/// can pin to it.
pub fn schema_version() -> &'static str {
    SCHEMA_VERSION
}
