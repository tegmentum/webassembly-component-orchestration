//! Benchmarks for the hot plan-handling paths: canonical CBOR encode/decode,
//! digest computation, and structure validation. Run with `cargo bench -p
//! compose-core`. Native-only (criterion is a dev-dependency), so the
//! wasm32-wasip2 library build is unaffected.
use std::hint::black_box;

use compose_core::blobs::BlobStore;
use compose_core::plan::{self, PlanValidator};
use compose_core::types::{ComponentSpec, ImportBinding, Linkage, PlanV1, Policy};
use criterion::{criterion_group, criterion_main, Criterion};

fn comp(id: &str, fill: u8) -> ComponentSpec {
    ComponentSpec {
        id: id.to_string(),
        digest: vec![fill; 32],
        source: None,
    }
}

fn binding(consumer: &str, provider: &str) -> ImportBinding {
    ImportBinding {
        consumer_id: Some(consumer.to_string()),
        import_name: "dep".to_string(),
        provider_id: provider.to_string(),
        export_name: "svc".to_string(),
    }
}

/// A representative multi-component plan with an acyclic binding graph.
fn sample_plan() -> PlanV1 {
    PlanV1 {
        version: "1".to_string(),
        root: "a".to_string(),
        components: vec![comp("a", 1), comp("b", 2), comp("c", 3)],
        bindings: vec![binding("a", "b"), binding("b", "c")],
        secrets: vec![],
        policy: Policy::default(),
        linkage: Linkage::Static,
    }
}

fn bench_plan(c: &mut Criterion) {
    let plan = sample_plan();
    let bytes = plan::serialize(&plan).expect("serialize");

    // validate_structure does no blob I/O, but the validator owns a store.
    let tmp = tempfile::tempdir().expect("tempdir");
    let validator = PlanValidator::new(
        BlobStore::new(tmp.path().to_path_buf(), 64 * 1024 * 1024).expect("blob store"),
    );

    c.bench_function("plan_serialize", |b| {
        b.iter(|| plan::serialize(black_box(&plan)).unwrap())
    });
    c.bench_function("plan_deserialize", |b| {
        b.iter(|| plan::deserialize(black_box(&bytes)).unwrap())
    });
    c.bench_function("plan_compute_digest", |b| {
        b.iter(|| plan::compute_plan_digest(black_box(&plan)).unwrap())
    });
    c.bench_function("plan_validate_structure", |b| {
        b.iter(|| validator.validate_structure(black_box(&plan)).unwrap())
    });
}

criterion_group!(benches, bench_plan);
criterion_main!(benches);
