use anyhow::Result;
use compose_host_wasmtime::{types::*, CompositorHost, HostConfig};
use tracing::info;

fn main() -> Result<()> {
    init_tracing();
    info!(host = "wasmtime", event = "boot", "compose host starting");

    // Create host with default config
    let config = HostConfig::default();
    let host = CompositorHost::new(config)?;

    info!(
        host = "wasmtime",
        event = "engine",
        component_model = true,
        "wasmtime engine configured for component model execution"
    );

    // Demo 1: Plan validation
    demo_plan_validation(&host)?;

    // Demo 2: Secret resolution
    demo_secret_resolution(&host)?;

    // Demo 3: Trust verification
    demo_trust_verification(&host)?;

    // Demo 4: Policy enforcement (M5)
    demo_policy_enforcement(&host)?;

    // Demo 5: Tenant isolation (M5)
    demo_tenant_isolation(&host)?;

    // Demo 6: Audit logging (M5)
    demo_audit_logging(&host)?;

    // Demo 7: Metrics collection (M6)
    demo_metrics_collection(&host)?;

    // Demo 8: Attestation (M6)
    demo_attestation(&host)?;

    info!(
        host = "wasmtime",
        event = "shutdown",
        "compose host shutting down"
    );

    Ok(())
}

/// Demonstrate plan validation
fn demo_plan_validation(host: &CompositorHost) -> Result<()> {
    info!("=== Demo 1: Plan Validation ===");

    // Create a simple test component
    let test_component_bytes = create_minimal_component();
    let component_digest = host.blobs.put(&test_component_bytes)?;

    info!(
        digest = hex::encode(&component_digest),
        "stored test component"
    );

    // Create a plan
    let plan = PlanV1 {
        version: "1".to_string(),
        root: "test-component".to_string(),
        components: vec![ComponentSpec {
            id: "test-component".to_string(),
            digest: component_digest.clone(),
            source: None,
        }],
        bindings: vec![],
        secrets: vec![],
        linkage: Default::default(),
        explicit_exports: vec![],
        policy: Policy::default(),
    };

    // Validate the plan
    let validator = host.plan_validator();
    match validator.validate(&plan) {
        Ok(_) => {
            info!("plan validation succeeded");
            let digest = validator.compute_digest(&plan)?;
            info!("plan digest: {}", hex::encode(&digest));
        }
        Err(e) => {
            info!("plan validation failed: {}", e);
        }
    }

    Ok(())
}

/// Demonstrate secret resolution
fn demo_secret_resolution(host: &CompositorHost) -> Result<()> {
    info!("=== Demo 2: Secret Resolution ===");

    // Secrets were pre-configured in the host initialization
    // Resolve secrets
    let api_key_token = host
        .secrets
        .resolve(&"api-key".to_string(), &"dev://".to_string())?;

    info!(token = %api_key_token, "resolved api-key secret");

    // Get metadata (without exposing the value)
    let metadata = host.secrets.get_metadata(&api_key_token)?;
    info!(
        secret_id = %metadata.id,
        backend = %metadata.backend,
        "secret metadata"
    );

    // List available secrets
    let secrets = host.secrets.list_secrets(Some(&"dev://".to_string()))?;
    info!(count = secrets.len(), "available secrets in dev backend");

    // Validate token
    let is_valid = host.secrets.validate_token(&api_key_token)?;
    info!(valid = is_valid, "token validation");

    Ok(())
}

/// Demonstrate trust verification
fn demo_trust_verification(host: &CompositorHost) -> Result<()> {
    info!("=== Demo 3: Trust Verification ===");

    // Create a test artifact
    let artifact_bytes = b"test artifact content";
    let digest = host.blobs.put(artifact_bytes)?;

    info!(digest = hex::encode(&digest), "created test artifact");

    // Create a dummy signature (in production, this would be real)
    let signature = b"{\"identity\": \"test@example.com\"}";

    // Initialize trust store
    let trust_store = compose_host_wasmtime::trust::TrustStore::new(
        std::path::PathBuf::from(".compose/trust"),
        compose_host_wasmtime::SystemClock::shared(),
    )?;

    // Verify using dev backend (no real verification)
    match trust_store.verify(&digest, artifact_bytes, Some(signature)) {
        Ok(result) => {
            info!(
                verified = result.verified,
                signer = %result.metadata.signer,
                backend = %result.metadata.backend,
                "verification succeeded"
            );
        }
        Err(e) => {
            info!("verification failed: {}", e);
        }
    }

    // Add to trusted set
    trust_store.trust_digest(
        &digest,
        compose_host_wasmtime::types::VerificationMetadata {
            signer: "test@example.com".to_string(),
            timestamp: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
            backend: "dev".to_string(),
        },
    )?;

    // Verify it's trusted
    let is_trusted = trust_store.is_trusted(&digest);
    info!(trusted = is_trusted, "artifact trust status");

    Ok(())
}

/// Create a minimal WebAssembly component for testing
fn create_minimal_component() -> Vec<u8> {
    // Minimal valid WebAssembly module
    // (module)
    vec![
        0x00, 0x61, 0x73, 0x6d, // magic
        0x01, 0x00, 0x00, 0x00, // version
    ]
}

/// Demonstrate policy enforcement with capability filtering (M5)
fn demo_policy_enforcement(host: &CompositorHost) -> Result<()> {
    info!("=== Demo 4: Policy Enforcement (M5) ===");

    // Test 1: Required capability denied
    info!("Test 1: Required capability denied");
    let plan_with_denied_cap = PlanV1 {
        version: "1".to_string(),
        root: "test".to_string(),
        components: vec![],
        bindings: vec![],
        secrets: vec![],
        linkage: Default::default(),
        explicit_exports: vec![],
        policy: Policy {
            determinism: DeterminismMode::Strict,
            capabilities: vec![
                Capability {
                    name: "wasi:cli".to_string(),
                    level: CapabilityLevel::Required,
                },
                Capability {
                    name: "forbidden:capability".to_string(),
                    level: CapabilityLevel::Required,
                },
            ],
            tenant: None,
            limits: ResourceLimits::default(),
        },
    };

    match host
        .policy_enforcer
        .enforce_policy(&plan_with_denied_cap.policy)
    {
        Ok(_) => info!("policy enforcement unexpectedly succeeded"),
        Err(e) => info!("policy enforcement correctly failed: {}", e),
    }

    // Test 2: Optional capability denied (soft degradation)
    info!("Test 2: Optional capability denied (soft degradation)");
    let plan_with_optional_cap = PlanV1 {
        version: "1".to_string(),
        root: "test".to_string(),
        components: vec![],
        bindings: vec![],
        secrets: vec![],
        linkage: Default::default(),
        explicit_exports: vec![],
        policy: Policy {
            determinism: DeterminismMode::Relaxed,
            capabilities: vec![
                Capability {
                    name: "wasi:cli".to_string(),
                    level: CapabilityLevel::Required,
                },
                Capability {
                    name: "experimental:feature".to_string(),
                    level: CapabilityLevel::Optional,
                },
            ],
            tenant: None,
            limits: ResourceLimits::default(),
        },
    };

    match host
        .policy_enforcer
        .enforce_policy(&plan_with_optional_cap.policy)
    {
        Ok(enforced) => {
            info!(
                allowed_caps = enforced.capabilities.len(),
                denied_optional = enforced.denied_optional.len(),
                "policy enforcement succeeded with degradation"
            );
            if !enforced.denied_optional.is_empty() {
                info!(
                    denied = ?enforced.denied_optional,
                    "optional capabilities denied"
                );
            }
        }
        Err(e) => info!("policy enforcement failed: {}", e),
    }

    // Test 3: Resource limit enforcement
    info!("Test 3: Resource limit enforcement");
    let plan_with_limits = Policy {
        determinism: DeterminismMode::Relaxed,
        capabilities: vec![Capability {
            name: "wasi:cli".to_string(),
            level: CapabilityLevel::Required,
        }],
        tenant: None,
        limits: ResourceLimits {
            cpu_ms: Some(100_000),                 // Exceeds host max (60s)
            memory_bytes: Some(256 * 1024 * 1024), // Within host max
            io_ops: Some(5_000),                   // Within host max
        },
    };

    match host
        .policy_enforcer
        .enforce_limits(&plan_with_limits.limits)
    {
        Ok(enforced) => {
            info!(
                cpu_ms = enforced.cpu_ms,
                memory_bytes = enforced.memory_bytes,
                io_ops = enforced.io_ops,
                "resource limits enforced (CPU capped to host max)"
            );
        }
        Err(e) => info!("limit enforcement failed: {}", e),
    }

    Ok(())
}

/// Demonstrate tenant-scoped cache isolation (M5)
fn demo_tenant_isolation(host: &CompositorHost) -> Result<()> {
    info!("=== Demo 5: Tenant Isolation (M5) ===");

    // Create a test component
    let test_component_bytes = create_minimal_component();
    let component_digest = host.blobs.put(&test_component_bytes)?;

    // Create two identical plans with different tenant IDs
    let plan_tenant_a = PlanV1 {
        version: "1".to_string(),
        root: "test-component".to_string(),
        components: vec![ComponentSpec {
            id: "test-component".to_string(),
            digest: component_digest.clone(),
            source: None,
        }],
        bindings: vec![],
        secrets: vec![],
        linkage: Default::default(),
        explicit_exports: vec![],
        policy: Policy {
            determinism: DeterminismMode::Strict,
            capabilities: vec![Capability {
                name: "wasi:cli".to_string(),
                level: CapabilityLevel::Required,
            }],
            tenant: Some("tenant-a".to_string()),
            limits: ResourceLimits::default(),
        },
    };

    let plan_tenant_b = PlanV1 {
        policy: Policy {
            tenant: Some("tenant-b".to_string()),
            ..plan_tenant_a.policy.clone()
        },
        ..plan_tenant_a.clone()
    };

    info!("executing plan for tenant-a");
    let exec_handler = host.exec_handler();
    match exec_handler.run_cli(&plan_tenant_a, vec![], vec![]) {
        Ok(result_a) => {
            info!(
                exec_key = hex::encode(&result_a.exec_key),
                exit_code = result_a.exit_code,
                "tenant-a execution complete"
            );

            info!("executing plan for tenant-b");
            match exec_handler.run_cli(&plan_tenant_b, vec![], vec![]) {
                Ok(result_b) => {
                    info!(
                        exec_key = hex::encode(&result_b.exec_key),
                        exit_code = result_b.exit_code,
                        "tenant-b execution complete"
                    );

                    // Verify different exec keys (cache isolation)
                    if result_a.exec_key != result_b.exec_key {
                        info!("✓ cache isolation verified: different tenant exec-keys");
                    } else {
                        info!("✗ cache isolation failed: same exec-key for different tenants");
                    }
                }
                Err(e) => {
                    info!(
                        "tenant-b execution failed (expected for minimal component): {}",
                        e
                    );
                }
            }
        }
        Err(e) => {
            info!(
                "tenant-a execution failed (expected for minimal component): {}",
                e
            );
            info!("tenant isolation is still demonstrated via policy enforcement and audit logs");
        }
    }

    Ok(())
}

/// Demonstrate audit logging with tenant isolation (M5)
fn demo_audit_logging(host: &CompositorHost) -> Result<()> {
    info!("=== Demo 6: Audit Logging (M5) ===");

    // Create a test component
    let test_component_bytes = create_minimal_component();
    let component_digest = host.blobs.put(&test_component_bytes)?;

    // Execute plans for different tenants to generate audit logs
    let tenants = vec!["acme-corp", "globex-inc", "initech"];

    for tenant in &tenants {
        let plan = PlanV1 {
            version: "1".to_string(),
            root: "test-component".to_string(),
            components: vec![ComponentSpec {
                id: "test-component".to_string(),
                digest: component_digest.clone(),
                source: None,
            }],
            bindings: vec![],
            secrets: vec![],
            linkage: Default::default(),
            explicit_exports: vec![],
            policy: Policy {
                determinism: DeterminismMode::Relaxed,
                capabilities: vec![Capability {
                    name: "wasi:cli".to_string(),
                    level: CapabilityLevel::Required,
                }],
                tenant: Some(tenant.to_string()),
                limits: ResourceLimits::default(),
            },
        };

        info!(tenant = %tenant, "executing plan");
        let exec_handler = host.exec_handler();
        match exec_handler.run_cli(&plan, vec![], vec![]) {
            Ok(result) => {
                info!(
                    tenant = %tenant,
                    exit_code = result.exit_code,
                    "execution complete"
                );
            }
            Err(e) => {
                info!(
                    tenant = %tenant,
                    error = %e,
                    "execution failed (expected for minimal component), audit log still generated"
                );
            }
        }
    }

    // Check audit log directories
    let audit_dir = std::path::PathBuf::from(".compose/audit");
    info!(
        audit_dir = %audit_dir.display(),
        "checking audit logs"
    );

    for tenant in &tenants {
        let tenant_dir = audit_dir.join(tenant);
        if tenant_dir.exists() {
            let log_files: Vec<_> = std::fs::read_dir(&tenant_dir)?
                .filter_map(|e| e.ok())
                .collect();
            info!(
                tenant = %tenant,
                log_files = log_files.len(),
                "tenant audit logs found"
            );
        } else {
            info!(
                tenant = %tenant,
                "no audit logs found (directory doesn't exist)"
            );
        }
    }

    Ok(())
}

/// Demonstrate metrics collection (M6)
fn demo_metrics_collection(host: &CompositorHost) -> Result<()> {
    info!("=== Demo 7: Metrics Collection (M6) ===");

    // Metrics have already been collected during previous demos
    // Let's query and display them

    // List all metrics
    let all_metrics = host.metrics.list(None, None, None);
    info!(total_metrics = all_metrics.len(), "total metrics collected");

    // List exec metrics
    let exec_metrics = host.metrics.list(Some("exec"), None, None);
    info!(exec_metrics = exec_metrics.len(), "execution metrics");

    // Get exec duration summary
    if let Some(summary) = host.metrics.summary(
        "exec.duration_ms",
        compose_host_wasmtime::metrics::AggregationPeriod::Minute,
        None,
    ) {
        info!(
            metric = "exec.duration_ms",
            count = summary.count,
            avg_ms = summary.avg,
            min_ms = summary.min,
            max_ms = summary.max,
            "execution duration summary"
        );
    }

    // List success/failure counts
    let success_count = host.metrics.list(Some("exec.success"), None, None).len();
    let failure_count = host.metrics.list(Some("exec.failure"), None, None).len();

    info!(
        success = success_count,
        failures = failure_count,
        "execution outcomes"
    );

    Ok(())
}

/// Demonstrate attestation signing and verification (M6)
fn demo_attestation(host: &CompositorHost) -> Result<()> {
    info!("=== Demo 8: Attestation (M6) ===");

    // Create a claim for attestation
    let plan_digest = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let artifact_digest = vec![9, 10, 11, 12, 13, 14, 15, 16];
    let exec_key = Some(vec![17, 18, 19, 20, 21, 22, 23, 24]);

    let claim = compose_host_wasmtime::attest::Claim {
        claim_type: "execution".to_string(),
        plan_digest: plan_digest.clone(),
        artifact_digest: artifact_digest.clone(),
        exec_key: exec_key.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        host_id: "wasmtime-host".to_string(),
        custom_claims: Some(r#"{"determinism":"strict","tenant":"test"}"#.to_string()),
    };

    info!(
        claim_type = %claim.claim_type,
        plan_digest = hex::encode(&claim.plan_digest),
        "creating attestation"
    );

    // Sign the claim
    let attestation = host
        .attestation
        .attest(claim, compose_host_wasmtime::attest::Algorithm::Ed25519)
        .map_err(|e| anyhow::anyhow!(e))?;

    info!(
        algorithm = ?attestation.algorithm,
        signature_len = attestation.signature.len(),
        "attestation created"
    );

    // Verify the attestation
    let verification = host
        .attestation
        .verify(&attestation)
        .map_err(|e| anyhow::anyhow!(e))?;

    info!(
        valid = verification.valid,
        signer = %verification.signer,
        "attestation verified"
    );

    // Export to JSON format
    let json_export = host
        .attestation
        .export(&attestation, "json")
        .map_err(|e| anyhow::anyhow!(e))?;
    info!(
        format = "json",
        size_bytes = json_export.len(),
        "attestation exported to JSON"
    );

    // Export to SLSA format
    let slsa_export = host
        .attestation
        .export(&attestation, "slsa")
        .map_err(|e| anyhow::anyhow!(e))?;
    info!(
        format = "slsa",
        size_bytes = slsa_export.len(),
        contains_intoto = slsa_export.contains("in-toto.io"),
        "attestation exported to SLSA provenance format"
    );

    // Get public key for verification
    let public_key = host
        .attestation
        .get_public_key(compose_host_wasmtime::attest::Algorithm::Ed25519)
        .map_err(|e| anyhow::anyhow!(e))?;

    info!(
        key_algorithm = "ed25519",
        key_size = public_key.len(),
        "host public key retrieved"
    );

    Ok(())
}

fn init_tracing() {
    // For now we just use the default subscriber so CLI output remains simple.
    let _ = tracing_subscriber::fmt::try_init();
}
