//! Round-trip + digest correctness tests for SqliteComposeStore.

use compose_core::types::{ComponentSpec, ImportBinding, PlanV1, Policy};
use compose_store_sqlite::{SqliteComposeStore, FORMAT_V1};

/// Fixed-shape plan fixture. Stable across runs so digest
/// assertions stay deterministic.
fn fixture_plan(name_suffix: &str) -> PlanV1 {
    PlanV1 {
        version: "1".into(),
        root: format!("root-{name_suffix}"),
        components: vec![ComponentSpec {
            id: format!("root-{name_suffix}"),
            digest: vec![1, 2, 3, 4, 5, 6, 7, 8],
            source: None,
        }],
        bindings: vec![ImportBinding {
            consumer_id: Some(format!("root-{name_suffix}")),
            import_name: "wasi:cli/run@0.2.6".into(),
            provider_id: "host".into(),
            export_name: "wasi:cli/run@0.2.6".into(),
        }],
        secrets: vec![],
        policy: Policy::default(),
        linkage: Default::default(),
    }
}

fn fresh() -> (tempfile::TempDir, SqliteComposeStore) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compose.sqlite");
    let store = SqliteComposeStore::open(&path).unwrap();
    (dir, store)
}

#[test]
fn put_get_roundtrips_a_plan() {
    let (_d, mut s) = fresh();
    let plan = fixture_plan("rt");
    s.put("my-plan", &plan).unwrap();
    let got = s.get("my-plan").unwrap().expect("row");
    assert_eq!(got.name, "my-plan");
    assert_eq!(got.version, "1");
    assert_eq!(got.format, FORMAT_V1);
    assert!(!got.body.is_empty());
    let plan2 = s.get_plan("my-plan").unwrap().expect("plan");
    assert_eq!(plan2.root, "root-rt");
    assert_eq!(plan2.components.len(), 1);
}

#[test]
fn put_replaces_on_same_name() {
    let (_d, mut s) = fresh();
    s.put("dup", &fixture_plan("v1")).unwrap();
    s.put("dup", &fixture_plan("v2")).unwrap();
    let plan = s.get_plan("dup").unwrap().expect("plan");
    assert_eq!(plan.root, "root-v2");
    assert_eq!(s.count().unwrap(), 1);
}

#[test]
fn list_orders_by_name() {
    let (_d, mut s) = fresh();
    s.put("z-last", &fixture_plan("z")).unwrap();
    s.put("a-first", &fixture_plan("a")).unwrap();
    s.put("m-middle", &fixture_plan("m")).unwrap();
    let names = s.list_names().unwrap();
    assert_eq!(names, vec!["a-first", "m-middle", "z-last"]);
}

#[test]
fn delete_returns_true_iff_row_existed() {
    let (_d, mut s) = fresh();
    s.put("doomed", &fixture_plan("d")).unwrap();
    assert!(s.delete("doomed").unwrap());
    assert!(!s.delete("doomed").unwrap());
    assert!(s.get("doomed").unwrap().is_none());
}

#[test]
fn digest_matches_compose_core() {
    use compose_core::plan::compute_plan_digest;
    let (_d, mut s) = fresh();
    let plan = fixture_plan("dig");
    s.put("digplan", &plan).unwrap();
    let got = s.get("digplan").unwrap().expect("row");
    let expected = hex::encode(compute_plan_digest(&plan).unwrap());
    assert_eq!(got.digest_hex, expected);
}

#[test]
fn get_by_digest_finds_the_row() {
    use compose_core::plan::compute_plan_digest;
    let (_d, mut s) = fresh();
    let plan = fixture_plan("bydig");
    s.put("name1", &plan).unwrap();
    let hex_digest = hex::encode(compute_plan_digest(&plan).unwrap());
    let row = s.get_by_digest(&hex_digest).unwrap().expect("row");
    assert_eq!(row.name, "name1");
    assert!(s
        .get_by_digest(&"00".repeat(32))
        .unwrap()
        .is_none());
}

#[test]
fn schema_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compose.sqlite");
    {
        let mut s = SqliteComposeStore::open(&path).unwrap();
        s.put("persist", &fixture_plan("p")).unwrap();
    }
    let s = SqliteComposeStore::open(&path).unwrap();
    let got = s.get_plan("persist").unwrap().expect("plan");
    assert_eq!(got.root, "root-p");
}
