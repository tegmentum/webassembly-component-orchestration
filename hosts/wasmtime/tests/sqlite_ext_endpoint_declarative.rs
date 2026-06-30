//! Task #219 — the wasmtime reference-host arm proving the REUSABLE
//! `sqlite-extension-endpoint` provider bridges DECLARATIVE
//! `sqlite:extension@1.0.0` tiers through `compose:dynlink`, end-to-end,
//! against REAL sqlink catalog extensions.
//!
//! One generic harness + one parameterized provider source; each test
//! loads a different (provider, scenario) pair built by
//! `sqlite-extension-endpoint/build.sh`. Mirrors the spike #218 test
//! (aba_dynlink_spike.rs): store + trust both digests, build a
//! runtime-linkage PlanV1 (harness = root, provider under id "ext"),
//! run through the real ExecHandler. Skips gracefully if artifacts
//! aren't built.

use compose_host_wasmtime::types::{
    Capability, CapabilityLevel, ComponentSpec, DeterminismMode, ImportBinding, Linkage, PlanV1,
    Policy, ResourceLimits,
};
use compose_host_wasmtime::{CompositorHost, HostConfig};
use std::path::PathBuf;

fn module_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sqlite-extension-endpoint")
}

fn provider_path(name: &str) -> PathBuf {
    module_dir().join("dist/providers").join(name)
}

fn harness_path() -> PathBuf {
    module_dir().join("harness/target/wasm32-wasip2/release/sqlite-ext-endpoint-harness.wasm")
}

/// Run the generic harness against `provider_file` with `SCENARIO=scenario`,
/// returning (exit_code, stdout, stderr).
fn run_tier(provider_file: &str, scenario: &str) -> Option<(u32, String, String)> {
    let (Ok(provider_bytes), Ok(harness_bytes)) =
        (std::fs::read(provider_path(provider_file)), std::fs::read(harness_path()))
    else {
        eprintln!("skipping: build artifacts first (sqlite-extension-endpoint/build.sh)");
        return None;
    };

    let tmp = tempfile::tempdir().expect("temp dir");
    let mut config = HostConfig::default();
    config.blob_dir = tmp.path().join("blobs");
    config.cache_dir = tmp.path().join("cache");
    config.trust_dir = tmp.path().join("trust");
    config.audit_dir = tmp.path().join("audit");
    let host = CompositorHost::new(config).expect("host");

    let provider_digest = host.blobs.put(&provider_bytes).expect("put provider");
    let harness_digest = host.blobs.put(&harness_bytes).expect("put harness");

    let meta = compose_host_wasmtime::types::VerificationMetadata {
        signer: "task-219".to_string(),
        timestamp: None,
        backend: "dev".to_string(),
    };
    host.trust.trust_digest(&provider_digest, meta.clone()).expect("trust provider");
    host.trust.trust_digest(&harness_digest, meta).expect("trust harness");

    let plan = PlanV1 {
        version: "1".to_string(),
        root: "harness".to_string(),
        components: vec![
            ComponentSpec { id: "harness".to_string(), digest: harness_digest, source: None },
            ComponentSpec { id: "ext".to_string(), digest: provider_digest, source: None },
        ],
        bindings: Vec::<ImportBinding>::new(),
        secrets: Vec::new(),
        policy: Policy {
            determinism: DeterminismMode::Relaxed,
            capabilities: vec![
                Capability { name: "dynlink:resolve".to_string(), level: CapabilityLevel::Required },
                Capability { name: "dynlink:invoke".to_string(), level: CapabilityLevel::Required },
            ],
            tenant: None,
            limits: ResourceLimits::default(),
        },
        linkage: Linkage::Runtime,
    };

    let exec = host.exec_handler();
    // SCENARIO selects the tier exercise inside the generic harness; it is
    // threaded into the GUEST's WASI environment (not the host process).
    let env = vec![("SCENARIO".to_string(), scenario.to_string())];
    let result = exec.run_cli(&plan, Vec::new(), env).expect("run");
    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();
    println!("==== tier {scenario} via {provider_file}: exit {} ====", result.exit_code);
    println!("STDOUT:\n{stdout}");
    if !stderr.trim().is_empty() {
        println!("STDERR:\n{stderr}");
    }
    Some((result.exit_code, stdout, stderr))
}

#[test]
fn tier_scalar_aba() {
    let Some((code, out, err)) = run_tier("aba-provider.wasm", "scalar") else { return };
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("loaded extension: aba"), "{out}");
    assert!(out.contains("aba_validate('021000021') => 1"), "{out}");
    assert!(out.contains("aba_validate('021000022') => 0"), "{out}");
}

#[test]
fn tier_aggregate_count_min() {
    let Some((code, out, err)) = run_tier("count_min-provider.wasm", "aggregate") else { return };
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("loaded extension: count_min"), "{out}");
    // The full aggregate lifecycle (step x5 + finalize) produced a sketch,
    // and the estimate scalar reports apple's count as >= 3 (the true
    // multiplicity; count-min never undercounts).
    assert!(out.contains("count_min(...)"), "{out}");
    assert!(out.contains("count_min_estimate(sketch, 'apple') => 3"), "{out}");
    assert!(out.contains("count_min_estimate(sketch, 'banana') => 1"), "{out}");
    // a value never inserted estimates 0 (no hash collision in this tiny set).
    assert!(out.contains("count_min_estimate(sketch, 'durian') => 0"), "{out}");
}

#[test]
fn tier_collation_uint() {
    let Some((code, out, err)) = run_tier("uint-provider.wasm", "collation") else { return };
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("loaded extension: uint"), "{out}");
    // Natural-numeric order: x2 < x10 (lexical would say x2 > x10).
    assert!(out.contains("compare('x2','x10') => -1") || out.contains("('x2' < 'x10')"), "{out}");
    assert!(out.contains("('x10' > 'x2')"), "{out}");
    assert!(out.contains("compare('x1','x1') => 0") || out.contains("('x1' == 'x1')"), "{out}");
}

#[test]
fn tier_vtab_series() {
    let Some((code, out, err)) = run_tier("series-provider.wasm", "vtab") else { return };
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("loaded extension: series"), "{out}");
    // The full read-vtab cursor surface (connect/open/filter/eof/column/next)
    // enumerated generate_series(1,5).
    assert!(out.contains("generate_series(1,5) => [1, 2, 3, 4, 5]"), "{out}");
}

#[test]
fn tier_vtab_mut_inmem() {
    // inmem is a self-contained mutable vtab (thread-local storage, world
    // tabular-mutating). The full mutating path runs: create -> xUpdate
    // INSERT x2 -> read back through the cursor surface.
    let Some((code, out, err)) = run_tier("inmem-provider.wasm", "vtab-mut") else { return };
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("loaded extension: inmem"), "{out}");
    assert!(out.contains("INSERT (key=alpha, value=100) => rowid"), "{out}");
    assert!(out.contains("SELECT key,value FROM inmem => [(alpha=100), (beta=200)]"), "{out}");
}

#[test]
fn tier_hooks_hookcb() {
    // hookcb is a purpose-built declarative hook extension (no reentrant
    // host-SPI), exercising the authorizer + update/commit/wal callback
    // surface. (The catalog hook extension hookprobe drags in
    // spi/wal-frames/s3-base via its reentrant scalar probes — that surface
    // is the reentrant tier #220's job.)
    let Some((code, out, err)) = run_tier("hookcb-provider.wasm", "hooks") else { return };
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("loaded extension: hookcb"), "{out}");
    assert!(out.contains("authorizer=true commit=true update=true wal=true"), "{out}");
    // authorizer: ordinary table allowed, "secret" denied.
    assert!(out.contains("authorize(read, arg1='t') => ok"), "{out}");
    assert!(out.contains("authorize(read, arg1='secret') => deny"), "{out}");
    // commit hook does not veto by default.
    assert!(out.contains("commit-hook on_commit() => veto=false"), "{out}");
    // wal hook returns SQLITE_OK.
    assert!(out.contains("=> rc=0"), "{out}");
}

#[test]
fn tier_dotcmd_dotret() {
    // dotret returns its output via invoke-result.text (no cli-stdout
    // streaming), so the provider composes without the cyclic cli-stdout
    // dependency. A STREAMING dot-command (greet) needs the host to wire
    // its cli-stdout import to the provider's cli-stdout export — see REPORT.
    let Some((code, out, err)) = run_tier("dotret-provider.wasm", "dotcmd") else { return };
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("loaded extension: dotret"), "{out}");
    assert!(out.contains("dot-command id=1 name=.echo"), "{out}");
    assert!(out.contains(r#".echo hello world => ok=true exit=0 text="echo: hello world""#), "{out}");
}
