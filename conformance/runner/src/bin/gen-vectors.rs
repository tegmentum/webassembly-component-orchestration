//! Regenerate the conformance plan vectors under `conformance/vectors/`.
//!
//! The vectors are canonical-CBOR `PlanV1` encodings consumed by the
//! conformance runner (which feeds each to `composectl plan validate` and
//! checks the outcome against `fixtures.json`). Run after any change to the
//! plan schema or validation rules so the corpus does not drift:
//!
//!   cargo run -p conformance-runner --bin gen-vectors
//!
//! Keep this in sync with `fixtures.json` (names + `expect_pass`).
use compose_host_wasmtime::blobs::compute_digest;
use compose_host_wasmtime::plan;
use compose_host_wasmtime::types::*;
use std::path::PathBuf;

fn comp(id: &str, fill: u8) -> ComponentSpec {
    ComponentSpec {
        id: id.to_string(),
        digest: vec![fill; 32],
        source: None,
    }
}

fn plan_v1(
    root: &str,
    components: Vec<ComponentSpec>,
    bindings: Vec<ImportBinding>,
    policy: Policy,
) -> PlanV1 {
    PlanV1 {
        version: "1".to_string(),
        root: root.to_string(),
        components,
        bindings,
        secrets: vec![],
        policy,
        linkage: Linkage::Static,
        explicit_exports: vec![],
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

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("conformance/vectors");

    let fixtures: Vec<(&str, PlanV1)> = vec![
        // hello-plan (pass): minimal single-component plan.
        (
            "hello-plan",
            plan_v1("app", vec![comp("app", 0x11)], vec![], Policy::default()),
        ),
        // multi-component-plan (pass): several components in canonical order.
        (
            "multi-component-plan",
            plan_v1(
                "a",
                vec![comp("a", 1), comp("b", 2), comp("c", 3)],
                vec![],
                Policy::default(),
            ),
        ),
        // nested-plan (pass): an acyclic dependency tree a -> b -> c.
        (
            "nested-plan",
            plan_v1(
                "a",
                vec![comp("a", 1), comp("b", 2), comp("c", 3)],
                vec![binding("a", "b"), binding("b", "c")],
                Policy::default(),
            ),
        ),
        // large-int-plan (pass): u64::MAX limits exercise big-int CBOR encoding.
        ("large-int-plan", {
            let mut policy = Policy::default();
            policy.limits.memory_bytes = Some(u64::MAX);
            policy.limits.cpu_ms = Some(u64::MAX);
            policy.limits.io_ops = Some(u64::MAX);
            plan_v1("app", vec![comp("app", 0x22)], vec![], policy)
        }),
        // duplicate-plan (FAIL): repeated component id.
        (
            "duplicate-plan",
            plan_v1(
                "dup",
                vec![comp("dup", 1), comp("dup", 2)],
                vec![],
                Policy::default(),
            ),
        ),
        // multi-component-plan-unsorted (FAIL): components out of canonical order.
        (
            "multi-component-plan-unsorted",
            plan_v1(
                "a",
                vec![comp("b", 2), comp("a", 1)],
                vec![],
                Policy::default(),
            ),
        ),
    ];

    for (name, p) in fixtures {
        let bytes = plan::serialize(&p).expect("serialize plan");
        std::fs::write(dir.join(format!("{name}.cbor")), &bytes).expect("write .cbor");
        let digest = compute_digest(&bytes);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(
            dir.join(format!("{name}.sha256.txt")),
            format!("{hex}  {name}.cbor\n"),
        )
        .expect("write .sha256.txt");
        println!("wrote {name}.cbor ({} bytes)", bytes.len());
    }
}
