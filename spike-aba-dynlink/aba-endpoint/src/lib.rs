//! `aba-endpoint` — a `compose:dynlink/endpoint` adapter that fronts the
//! real `aba` `sqlite:extension` component.
//!
//! This is the SPIKE's PROVIDER MODEL. A scalar `sqlite:extension`
//! component (aba) exports `sqlite:extension/metadata` (`describe`) and
//! `sqlite:extension/scalar-function` (`call`). compose:dynlink providers
//! instead export the single uniform `compose:dynlink/endpoint.handle`
//! (bytes-in / bytes-out, CBOR envelope). This adapter bridges the two:
//!
//!   * it EXPORTS `compose:dynlink/endpoint`  (valid dynlink provider)
//!   * it IMPORTS `sqlite:extension/metadata` + `/scalar-function`,
//!     satisfied at compose time by the aba component (which exports them).
//!
//! The adapter is "the SQLite connection behind the provider": it is the
//! minimal SQLite host-SPI surface a scalar extension needs (describe +
//! scalar dispatch), modeled exactly as the s3-endpoint / gdal-endpoint
//! resident providers are. CBOR envelope, mirroring s3-endpoint:
//!
//!   * `manifest` / `describe` -> CBOR manifest (registered scalars)
//!   * `call` -> CBOR { func_id, args } -> CBOR sql-value (the dispatch)

wit_bindgen::generate!({
    world: "aba-provider",
    path: "wit",
    generate_all,
});

use exports::compose::dynlink::endpoint::{Error, Guest};
use sys::compose::types::ErrorCode;

// Imported aba SPI (satisfied by the composed aba component).
use sqlite::extension::metadata as aba_meta;
use sqlite::extension::scalar_function as aba_scalar;
use sqlite::extension::types::SqlValue as WitSqlValue;

use serde::{Deserialize, Serialize};

/// CBOR-friendly mirror of `sqlite:extension/types.sql-value`. Tagged so
/// the wire form is self-describing (the dynlink "CBOR envelope" convention).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "lowercase")]
enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl From<&SqlValue> for WitSqlValue {
    fn from(v: &SqlValue) -> WitSqlValue {
        match v {
            SqlValue::Null => WitSqlValue::Null,
            SqlValue::Integer(i) => WitSqlValue::Integer(*i),
            SqlValue::Real(r) => WitSqlValue::Real(*r),
            SqlValue::Text(s) => WitSqlValue::Text(s.clone()),
            SqlValue::Blob(b) => WitSqlValue::Blob(b.clone()),
        }
    }
}

impl From<WitSqlValue> for SqlValue {
    fn from(v: WitSqlValue) -> SqlValue {
        match v {
            WitSqlValue::Null => SqlValue::Null,
            WitSqlValue::Integer(i) => SqlValue::Integer(i),
            WitSqlValue::Real(r) => SqlValue::Real(r),
            WitSqlValue::Text(s) => SqlValue::Text(s),
            WitSqlValue::Blob(b) => SqlValue::Blob(b),
        }
    }
}

/// One registered scalar, as surfaced over the envelope.
#[derive(Debug, Serialize, Deserialize)]
struct ScalarSpec {
    id: u64,
    name: String,
    num_args: i32,
}

/// The describe response: the extension's name/version + its scalars.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    name: String,
    version: String,
    scalars: Vec<ScalarSpec>,
}

/// The `call` request envelope.
#[derive(Debug, Serialize, Deserialize)]
struct CallReq {
    func_id: u64,
    args: Vec<SqlValue>,
}

struct AbaEndpoint;

fn err(code: ErrorCode, message: String) -> Error {
    Error {
        code,
        message,
        context: None,
    }
}

fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(v, &mut out)
        .map_err(|e| err(ErrorCode::InternalError, format!("cbor encode: {e}")))?;
    Ok(out)
}

fn decode<T: serde::de::DeserializeOwned>(payload: &[u8]) -> Result<T, Error> {
    ciborium::de::from_reader(payload)
        .map_err(|e| err(ErrorCode::InvalidInput, format!("cbor decode: {e}")))
}

impl Guest for AbaEndpoint {
    fn handle(method: String, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
        match method.as_str() {
            // Drive the loaded extension's describe() and surface the
            // registered scalar table. This is exactly the step the bespoke
            // host-side extension-loader performs (call describe, read the
            // manifest), here done over the dynlink endpoint.
            "manifest" | "describe" => {
                let m = aba_meta::describe();
                let scalars = m
                    .scalar_functions
                    .iter()
                    .map(|s| ScalarSpec {
                        id: s.id,
                        name: s.name.clone(),
                        num_args: s.num_args,
                    })
                    .collect();
                encode(&Manifest {
                    name: m.name,
                    version: m.version,
                    scalars,
                })
            }
            // The scalar dispatch: decode {func_id, args}, call the
            // extension's scalar-function.call, return the sql-value.
            "call" => {
                let req: CallReq = decode(&payload)?;
                let wit_args: Vec<WitSqlValue> = req.args.iter().map(WitSqlValue::from).collect();
                match aba_scalar::call(req.func_id, &wit_args) {
                    Ok(v) => {
                        let out: SqlValue = v.into();
                        encode(&out)
                    }
                    Err(msg) => Err(err(ErrorCode::ExecTrap, format!("scalar call failed: {msg}"))),
                }
            }
            other => Err(err(
                ErrorCode::InvalidInput,
                format!("unknown method: {other}"),
            )),
        }
    }
}

export!(AbaEndpoint);
