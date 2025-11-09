/// Conformance suite runner binary
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use conformance_runner::{create_default_tests, ConformanceReport, TestSuite, WasmtimeAdapter};
use std::fs;

#[derive(Parser)]
#[command(name = "conformance", about = "Run conformance tests against host implementations")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run conformance test suite
    Run {
        /// Host implementation to test (currently only 'wasmtime' supported)
        #[arg(long, default_value = "wasmtime")]
        host: String,

        /// Output file for JSON report
        #[arg(long)]
        output: Option<String>,

        /// Generate attestation for the report
        #[arg(long)]
        attest: bool,
    },

    /// List available test cases
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            host,
            output,
            attest,
        } => run_conformance(&host, output.as_deref(), attest),
        Command::List => list_tests(),
    }
}

fn run_conformance(host_id: &str, output_path: Option<&str>, attest: bool) -> Result<()> {
    println!("Running conformance suite against host: {}", host_id);

    // Currently only supports wasmtime
    if host_id != "wasmtime" {
        anyhow::bail!("Only 'wasmtime' host is currently supported");
    }

    // Create host adapter
    let adapter =
        WasmtimeAdapter::new().context("Failed to create wasmtime host adapter")?;

    // Create test suite
    let test_cases = create_default_tests();
    let suite = TestSuite::new(test_cases);

    // Run tests
    println!("Running {} test cases...", suite.test_cases.len());
    let mut report = suite.run(&adapter, host_id.to_string())?;

    // Add attestation if requested
    if attest {
        println!("Generating attestation...");
        let attestation = generate_attestation(&adapter, &report)?;
        report = report.with_attestation(attestation);
    }

    // Print summary
    println!("\n=== Conformance Report ===");
    println!("Host: {}", report.host_id);
    println!("Total tests: {}", report.total_tests);
    println!("Passed: {}", report.passed);
    println!("Failed: {}", report.failed);
    println!("Metrics collected: {}", report.metrics_count);
    println!("Audit records: {}", report.audit_count);

    if !report.all_passed() {
        println!("\nFailed tests:");
        for result in &report.results {
            if !result.passed {
                println!(
                    "  - {} ({:?}): {}",
                    result.name,
                    result.phase,
                    result.error.as_deref().unwrap_or("unknown error")
                );
            }
        }
    }

    // Write JSON report if requested
    if let Some(path) = output_path {
        let json = serde_json::to_string_pretty(&report)
            .context("Failed to serialize conformance report")?;
        fs::write(path, json).with_context(|| format!("Failed to write report to {}", path))?;
        println!("\nReport written to: {}", path);
    }

    if report.all_passed() {
        println!("\n✓ All conformance tests passed!");
        Ok(())
    } else {
        anyhow::bail!("{} test(s) failed", report.failed);
    }
}

fn generate_attestation(
    adapter: &WasmtimeAdapter,
    report: &ConformanceReport,
) -> Result<String> {
    use compose_host_wasmtime::attest::{Algorithm, Claim};

    // Create a claim for the conformance report
    let report_json = serde_json::to_string(report)?;
    let report_digest = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(report_json.as_bytes());
        hasher.finalize().to_vec()
    };

    let claim = Claim {
        claim_type: "conformance".to_string(),
        plan_digest: report_digest.clone(),
        artifact_digest: report_digest,
        exec_key: None,
        timestamp: report.timestamp,
        host_id: report.host_id.clone(),
        custom_claims: Some(format!(
            r#"{{"passed":{},"total":{}}}"#,
            report.passed, report.total_tests
        )),
    };

    let attestation = adapter
        .host()
        .attestation
        .attest(claim, Algorithm::Ed25519)
        .map_err(|e| anyhow::anyhow!(e))?;

    adapter
        .host()
        .attestation
        .export(&attestation, "json")
        .map_err(|e| anyhow::anyhow!(e))
}

fn list_tests() -> Result<()> {
    let test_cases = create_default_tests();

    println!("Available conformance tests:\n");
    for test in &test_cases {
        println!("  {} ({:?})", test.name, test.phase);
        println!("    {}", test.description);
        println!(
            "    Expected: {}",
            if test.expect_success {
                "success"
            } else {
                "failure"
            }
        );
        println!();
    }

    println!("Total: {} tests", test_cases.len());
    Ok(())
}
