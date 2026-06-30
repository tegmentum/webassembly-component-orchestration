//! A CLI harness (flavor B: guest-driven dlopen) that proves the aba
//! scalar extension loads + dispatches through `compose:dynlink`.
//!
//! It asks the host's `compose:dynlink/linker` to resolve the aba
//! provider by id ("aba"), then speaks the CBOR envelope over the
//! provider's `endpoint`:
//!   1. `describe` -> read the registered scalar table, find `aba_validate`
//!   2. `call { func_id, args: [text(routing)] }` -> the boolean result
//!
//! Prints one line per probed routing number:  <routing> => <0|1>
//! exactly as a SQL `SELECT aba_validate(<routing>)` would.

wit_bindgen::generate!({
    world: "dynlink-guest",
    path: "wit",
    generate_all,
});

use compose::dynlink::linker::resolve_by_id;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "lowercase")]
enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Debug, Serialize, Deserialize)]
struct ScalarSpec {
    id: u64,
    name: String,
    num_args: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    name: String,
    version: String,
    scalars: Vec<ScalarSpec>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CallReq {
    func_id: u64,
    args: Vec<SqlValue>,
}

fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(v, &mut out).expect("cbor encode");
    out
}

fn die(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn main() {
    // 1. dlopen the aba provider by id (registered from the plan).
    let instance = match resolve_by_id("aba") {
        Ok(i) => i,
        Err(e) => die(format!("resolve(aba) failed: {}", e.message)),
    };

    // 2. describe(): obtain the registered scalar table.
    let manifest_bytes = match instance.invoke("describe", &[]) {
        Ok(b) => b,
        Err(e) => die(format!("describe failed: {}", e.message)),
    };
    let manifest: Manifest =
        ciborium::de::from_reader(&manifest_bytes[..]).expect("decode manifest");
    println!(
        "loaded extension: {} v{} ({} scalars)",
        manifest.name,
        manifest.version,
        manifest.scalars.len()
    );
    for s in &manifest.scalars {
        println!("  scalar id={} name={} num_args={}", s.id, s.name, s.num_args);
    }

    let aba_validate = manifest
        .scalars
        .iter()
        .find(|s| s.name == "aba_validate")
        .unwrap_or_else(|| die("aba_validate not in manifest".to_string()));

    // 3. dispatch aba_validate(routing) for a few known inputs.
    //    021000021 = JPMorgan Chase (valid); 021000022 = bad check digit.
    for routing in ["021000021", "021000022", "not a routing"] {
        let req = CallReq {
            func_id: aba_validate.id,
            args: vec![SqlValue::Text(routing.to_string())],
        };
        let resp = match instance.invoke("call", &encode(&req)) {
            Ok(b) => b,
            Err(e) => die(format!("call failed: {}", e.message)),
        };
        let val: SqlValue = ciborium::de::from_reader(&resp[..]).expect("decode result");
        let shown = match val {
            SqlValue::Integer(i) => i.to_string(),
            SqlValue::Null => "NULL".to_string(),
            other => format!("{other:?}"),
        };
        println!("aba_validate('{routing}') => {shown}");
    }
}
