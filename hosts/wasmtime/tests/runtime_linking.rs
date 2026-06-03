//! End-to-end test for flavor A (runtime linking) through the normal
//! `ExecHandler::run_cli` path: a `linkage = Runtime` plan runs its root
//! consumer command with the `compose:dynlink/endpoint` import bound to a
//! provider resolved (and trust-gated) at exec time.
//!
//! Skips gracefully unless both example components are built:
//!   examples/dynlink-echo-provider/build.sh
//!   examples/dynlink-endpoint-consumer/build.sh
use std::path::PathBuf;

use compose_host_wasmtime::{
    Capability, CapabilityLevel, ComponentSpec, CompositorHost, DeterminismMode, HostConfig,
    ImportBinding, Linkage, PlanV1, Policy, VerificationMetadata,
};

/// A policy that grants the two dynlink verbs (and is non-strict), as a
/// runtime-linked plan must.
fn runtime_policy() -> Policy {
    Policy {
        determinism: DeterminismMode::Relaxed,
        capabilities: vec![
            Capability {
                name: "dynlink:resolve".to_string(),
                level: CapabilityLevel::Required,
            },
            Capability {
                name: "dynlink:invoke".to_string(),
                level: CapabilityLevel::Required,
            },
        ],
        ..Policy::default()
    }
}

fn example(rel: &str, file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(rel)
        .join("target/wasm32-wasip2/release")
        .join(file)
}

#[test]
fn runtime_linked_plan_runs_consumer_with_bound_provider() {
    let (Ok(consumer), Ok(provider)) = (
        std::fs::read(example(
            "dynlink-endpoint-consumer",
            "dynlink-endpoint-consumer.wasm",
        )),
        std::fs::read(example(
            "dynlink-echo-provider",
            "dynlink_echo_provider.wasm",
        )),
    ) else {
        eprintln!(
            "skipping: build examples/dynlink-endpoint-consumer and \
             examples/dynlink-echo-provider first"
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("temp dir");
    let config = HostConfig {
        blob_dir: tmp.path().join("blobs"),
        cache_dir: tmp.path().join("cache"),
        trust_dir: tmp.path().join("trust"),
        audit_dir: tmp.path().join("audit"),
        max_blob_size: 64 * 1024 * 1024,
        attest_pkcs11: None,
    };
    let host = CompositorHost::new(config).expect("host");

    // Stage both components in the CAS and trust the provider digest.
    let consumer_digest = host.blobs.put(&consumer).expect("put consumer");
    let provider_digest = host.blobs.put(&provider).expect("put provider");
    host.trust
        .trust_digest(
            &provider_digest,
            VerificationMetadata {
                signer: "test".to_string(),
                timestamp: None,
                backend: "dev".to_string(),
            },
        )
        .expect("trust provider");

    let plan = PlanV1 {
        version: "1".to_string(),
        root: "consumer".to_string(),
        components: vec![
            ComponentSpec {
                id: "consumer".to_string(),
                digest: consumer_digest,
                source: None,
            },
            ComponentSpec {
                id: "provider".to_string(),
                digest: provider_digest,
                source: None,
            },
        ],
        bindings: vec![ImportBinding {
            consumer_id: Some("consumer".to_string()),
            import_name: "compose:dynlink/endpoint".to_string(),
            provider_id: "provider".to_string(),
            export_name: "compose:dynlink/endpoint".to_string(),
        }],
        secrets: vec![],
        policy: runtime_policy(),
        linkage: Linkage::Runtime,
    };

    let result = host
        .exec_handler()
        .run_cli(&plan, vec![], vec![])
        .expect("runtime-linked run");

    assert_eq!(
        result.exit_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        "HELLO FROM CONSUMER"
    );
}

#[test]
fn runtime_linked_plan_rejects_untrusted_provider() {
    let (Ok(consumer), Ok(provider)) = (
        std::fs::read(example(
            "dynlink-endpoint-consumer",
            "dynlink-endpoint-consumer.wasm",
        )),
        std::fs::read(example(
            "dynlink-echo-provider",
            "dynlink_echo_provider.wasm",
        )),
    ) else {
        return;
    };

    let tmp = tempfile::tempdir().expect("temp dir");
    let config = HostConfig {
        blob_dir: tmp.path().join("blobs"),
        cache_dir: tmp.path().join("cache"),
        trust_dir: tmp.path().join("trust"),
        audit_dir: tmp.path().join("audit"),
        max_blob_size: 64 * 1024 * 1024,
        attest_pkcs11: None,
    };
    let host = CompositorHost::new(config).expect("host");

    let consumer_digest = host.blobs.put(&consumer).expect("put consumer");
    // Provider is staged but NOT trusted.
    let provider_digest = host.blobs.put(&provider).expect("put provider");

    let plan = PlanV1 {
        version: "1".to_string(),
        root: "consumer".to_string(),
        components: vec![
            ComponentSpec {
                id: "consumer".to_string(),
                digest: consumer_digest,
                source: None,
            },
            ComponentSpec {
                id: "provider".to_string(),
                digest: provider_digest,
                source: None,
            },
        ],
        bindings: vec![ImportBinding {
            consumer_id: Some("consumer".to_string()),
            import_name: "compose:dynlink/endpoint".to_string(),
            provider_id: "provider".to_string(),
            export_name: "compose:dynlink/endpoint".to_string(),
        }],
        secrets: vec![],
        policy: runtime_policy(),
        linkage: Linkage::Runtime,
    };

    let err = host
        .exec_handler()
        .run_cli(&plan, vec![], vec![])
        .expect_err("untrusted provider must be rejected");
    assert_eq!(
        err.code,
        compose_host_wasmtime::ErrorCode::TrustUntrustedSource
    );
}
