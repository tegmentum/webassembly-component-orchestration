//! SPIKE (#218): prove that `compose:dynlink` can dynamically link a real
//! sqlink `sqlite:extension` scalar extension (`aba`) — modeled as a
//! resident `compose:dynlink/endpoint` provider — and dispatch a scalar
//! through it end-to-end on the wasmtime reference host.
//!
//! This is the wasmtime arm of the spike. It mirrors what `composectl
//! exec run` does for a `linkage: runtime`, guest-driven (flavor B) plan,
//! but drives it in-process so the digests can be trusted programmatically
//! (the CLI has no `trust add` and the on-disk trust store is not loaded at
//! startup). The flow is identical to the production exec path:
//!
//!   1. store the composed `aba-provider` (endpoint adapter + aba) and the
//!      `aba-dlopen-harness` guest in the blob store,
//!   2. trust both digests,
//!   3. build a runtime-linkage PlanV1 (harness = root, provider registered
//!      under id "aba"),
//!   4. run it through the real ExecHandler, which uses the
//!      `compose:dynlink` linker host bridge to resolve + invoke the
//!      provider.
//!
//! Skips gracefully if the spike artifacts aren't built.

use compose_host_wasmtime::types::{
    Capability, CapabilityLevel, ComponentSpec, DeterminismMode, ImportBinding, Linkage, PlanV1,
    Policy, ResourceLimits,
};
use compose_host_wasmtime::{CompositorHost, HostConfig};
use std::path::PathBuf;

fn spike_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spike-aba-dynlink")
}

fn provider_path() -> PathBuf {
    spike_dir().join("aba-provider.wasm")
}

fn harness_path() -> PathBuf {
    spike_dir().join("harness/target/wasm32-wasip2/release/aba-dlopen-harness.wasm")
}

#[test]
fn aba_loads_and_dispatches_through_dynlink() {
    let (Ok(provider_bytes), Ok(harness_bytes)) = (
        std::fs::read(provider_path()),
        std::fs::read(harness_path()),
    ) else {
        eprintln!(
            "skipping: build the spike artifacts first \
             (spike-aba-dynlink/build.sh)"
        );
        return;
    };

    // Isolated host rooted at a temp dir (own blob + trust + cache stores),
    // so the spike never touches the repo's `.compose`.
    let tmp = tempfile::tempdir().expect("temp dir");
    let mut config = HostConfig::default();
    config.blob_dir = tmp.path().join("blobs");
    config.cache_dir = tmp.path().join("cache");
    config.trust_dir = tmp.path().join("trust");
    config.audit_dir = tmp.path().join("audit");
    let host = CompositorHost::new(config).expect("host");

    // 1. Store both components.
    let provider_digest = host.blobs.put(&provider_bytes).expect("put provider");
    let harness_digest = host.blobs.put(&harness_bytes).expect("put harness");

    // 2. Trust both digests (the dynlink resolve path is trust-gated).
    let meta = compose_host_wasmtime::types::VerificationMetadata {
        signer: "spike".to_string(),
        timestamp: None,
        backend: "dev".to_string(),
    };
    host.trust
        .trust_digest(&provider_digest, meta.clone())
        .expect("trust provider");
    host.trust
        .trust_digest(&harness_digest, meta)
        .expect("trust harness");

    // 3. Build the runtime-linkage plan. Root = harness (imports the
    //    dynlink linker, flavor B); provider registered under id "aba" via
    //    the components table; granted both dynlink verbs.
    let plan = PlanV1 {
        version: "1".to_string(),
        root: "harness".to_string(),
        components: vec![
            ComponentSpec {
                id: "harness".to_string(),
                digest: harness_digest.clone(),
                source: None,
            },
            ComponentSpec {
                id: "aba".to_string(),
                digest: provider_digest.clone(),
                source: None,
            },
        ],
        bindings: Vec::<ImportBinding>::new(),
        secrets: Vec::new(),
        policy: Policy {
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
            tenant: None,
            limits: ResourceLimits::default(),
        },
        linkage: Linkage::Runtime,
    };

    // 4. Run it through the real exec handler (the production path).
    let exec = host.exec_handler();
    let result = exec.run_cli(&plan, Vec::new(), Vec::new()).expect("run");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    println!("---- exit {} ----", result.exit_code);
    println!("STDOUT:\n{stdout}");
    if !stderr.trim().is_empty() {
        println!("STDERR:\n{stderr}");
    }

    assert_eq!(result.exit_code, 0, "harness failed; stderr: {stderr}");
    // The describe() round-trip surfaced the registered scalar.
    assert!(
        stdout.contains("aba_validate"),
        "describe did not surface aba_validate; stdout: {stdout}"
    );
    // The scalar dispatch returned the right answers.
    assert!(
        stdout.contains("aba_validate('021000021') => 1"),
        "valid routing should return 1; stdout: {stdout}"
    );
    assert!(
        stdout.contains("aba_validate('021000022') => 0"),
        "bad check digit should return 0; stdout: {stdout}"
    );
}
