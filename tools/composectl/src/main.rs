/// Composectl - CLI for the compositional WebAssembly system
mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use cli::*;
use compose_host_wasmtime::{CompositorHost, HostConfig};
use std::fs;
use std::path::PathBuf;

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Initialize host
    let config = HostConfig::default();
    let host = CompositorHost::new(config)?;

    match cli.command {
        Commands::Plan { action } => handle_plan(&host, action, &cli.format),
        Commands::Emit { action } => handle_emit(&host, action, &cli.format),
        Commands::Exec { action } => handle_exec(&host, action, &cli.format),
        Commands::Secrets { action } => handle_secrets(&host, action, &cli.format),
        Commands::Trust { action } => handle_trust(&host, action, &cli.format),
        Commands::Reflect { action } => handle_reflect(&host, action, &cli.format),
        Commands::Metrics { action } => handle_metrics(&host, action, &cli.format),
        Commands::Blob { action } => handle_blob(&host, action, &cli.format),
    }
}

fn handle_plan(host: &CompositorHost, action: PlanAction, format: &OutputFormat) -> Result<()> {
    match action {
        PlanAction::Validate { plan } => {
            // Read and parse plan
            let plan_data = read_plan(&plan)?;

            // Validate using host
            let validator = host.plan_validator();
            validator.validate(&plan_data)?;

            output(format, "Plan is valid", &plan_data)?;
            Ok(())
        }
        PlanAction::Info { plan } => {
            let plan_data = read_plan(&plan)?;

            // Show plan information
            output(format, "Plan information", &plan_data)?;
            Ok(())
        }
    }
}

fn handle_emit(host: &CompositorHost, action: EmitAction, format: &OutputFormat) -> Result<()> {
    match action {
        EmitAction::Build { plan, output } => {
            let plan_data = read_plan(&plan)?;

            // Compose using host
            let emit_handler = compose_host_wasmtime::emit::EmitHandler::new(
                host.blobs.clone(),
                host.events.clone(),
                host.config.cache_dir.clone(),
            );

            let result = emit_handler.compose(&plan_data)?;

            // Get the composed bytes
            let composed_bytes = host.blobs.get(&result.digest)?;

            // Write to output file
            fs::write(&output, composed_bytes)
                .with_context(|| format!("Failed to write output to {:?}", output))?;

            println!("Artifact written to: {:?}", output);
            println!("Digest: {}", hex::encode(&result.digest));
            Ok(())
        }
    }
}

fn handle_exec(host: &CompositorHost, action: ExecAction, format: &OutputFormat) -> Result<()> {
    match action {
        ExecAction::Run { plan, args } => {
            let plan_data = read_plan(&plan)?;

            // Execute using host
            let exec_handler = host.exec_handler();
            let result = exec_handler.run_cli(&plan_data, args, vec![])?;

            if result.exit_code != 0 {
                eprintln!("Exit code: {}", result.exit_code);
                std::process::exit(result.exit_code as i32);
            }

            Ok(())
        }
        ExecAction::Invoke { plan, export } => {
            let plan_data = read_plan(&plan)?;

            println!("Invoking export '{}' from plan", export);
            println!("(Full invoke implementation requires WIT interface introspection)");
            Ok(())
        }
    }
}

fn handle_secrets(
    host: &CompositorHost,
    action: SecretsAction,
    format: &OutputFormat,
) -> Result<()> {
    match action {
        SecretsAction::List { backend } => {
            println!("Listing secrets from backend: {:?}", backend.unwrap_or_else(|| "default".to_string()));
            println!("(Secrets are managed via secret bindings in plans)");
            Ok(())
        }
        SecretsAction::Resolve { id, backend } => {
            println!("Resolving secret '{}' from backend '{}'", id, backend);
            println!("(Secret resolution happens at composition/execution time)");
            Ok(())
        }
    }
}

fn handle_trust(host: &CompositorHost, action: TrustAction, format: &OutputFormat) -> Result<()> {
    match action {
        TrustAction::Verify {
            artifact,
            signature,
        } => {
            println!("Verifying artifact: {:?}", artifact);
            if let Some(sig) = signature {
                println!("Using signature: {:?}", sig);
            }
            println!("(Trust verification requires trust backend configuration)");
            Ok(())
        }
        TrustAction::List => {
            println!("Listing trusted artifacts");
            println!("(Trust registry not yet implemented)");
            Ok(())
        }
    }
}

fn handle_reflect(
    host: &CompositorHost,
    action: ReflectAction,
    format: &OutputFormat,
) -> Result<()> {
    match action {
        ReflectAction::Exports { plan } => {
            let plan_data = read_plan(&plan)?;
            println!("Exports from plan:");
            println!("(Requires WIT interface introspection)");
            Ok(())
        }
        ReflectAction::Describe { plan, export } => {
            let plan_data = read_plan(&plan)?;
            println!("Describing export '{}' from plan", export);
            println!("(Requires WIT interface introspection)");
            Ok(())
        }
    }
}

fn handle_metrics(
    host: &CompositorHost,
    action: MetricsAction,
    format: &OutputFormat,
) -> Result<()> {
    match action {
        MetricsAction::List { filter, since } => {
            // List all metrics, filtering handled separately if needed
            let metrics = host.metrics.list(filter.as_deref(), None, None);

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&metrics)?);
                }
                OutputFormat::Text => {
                    println!("Collected Metrics:");
                    for metric in &metrics {
                        println!("  {} = {:?} @ {}", metric.name, metric.value, metric.timestamp);
                    }
                    println!("\nTotal: {} metrics", metrics.len());
                }
                OutputFormat::Toml => {
                    println!("{}", toml::to_string_pretty(&metrics)?);
                }
            }
            Ok(())
        }
        MetricsAction::Summary { name } => {
            use compose_host_wasmtime::metrics::AggregationPeriod;

            if let Some(summary) = host.metrics.summary(&name, AggregationPeriod::Day, None) {
                match format {
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&summary)?);
                    }
                    OutputFormat::Text => {
                        println!("Summary for metric '{}':", name);
                        println!("  Count: {}", summary.count);
                        println!("  Sum: {}", summary.sum);
                        println!("  Min: {}", summary.min);
                        println!("  Max: {}", summary.max);
                        println!("  Avg: {}", summary.avg);
                    }
                    OutputFormat::Toml => {
                        println!("{}", toml::to_string_pretty(&summary)?);
                    }
                }
            } else {
                println!("No metrics found for '{}'", name);
            }
            Ok(())
        }
    }
}

fn handle_blob(host: &CompositorHost, action: BlobAction, format: &OutputFormat) -> Result<()> {
    match action {
        BlobAction::Put { file } => {
            // Read file
            let bytes = fs::read(&file)
                .with_context(|| format!("Failed to read file {:?}", file))?;

            // Store in blob store
            let digest = host.blobs.put(&bytes)?;

            println!("Blob stored successfully");
            println!("  File: {:?}", file);
            println!("  Size: {} bytes", bytes.len());
            println!("  Digest: {}", hex::encode(&digest));
            Ok(())
        }
        BlobAction::Get { digest, output } => {
            // Parse hex digest
            let digest_bytes = hex::decode(&digest)
                .with_context(|| format!("Invalid hex digest: {}", digest))?;

            // Get blob
            let bytes = host.blobs.get(&digest_bytes)?;

            // Write to output
            fs::write(&output, bytes)
                .with_context(|| format!("Failed to write to {:?}", output))?;

            println!("Blob retrieved successfully");
            println!("  Digest: {}", digest);
            println!("  Output: {:?}", output);
            Ok(())
        }
        BlobAction::Has { digest } => {
            // Parse hex digest
            let digest_bytes = hex::decode(&digest)
                .with_context(|| format!("Invalid hex digest: {}", digest))?;

            // Check existence
            let exists = host.blobs.has(&digest_bytes);

            println!("Blob {} found: {}", digest, exists);
            Ok(())
        }
        BlobAction::List => {
            // List all blobs
            let blobs = host.blobs.list_all();

            match format {
                OutputFormat::Json => {
                    let hex_digests: Vec<String> = blobs.iter().map(|d| hex::encode(d)).collect();
                    println!("{}", serde_json::to_string_pretty(&hex_digests)?);
                }
                OutputFormat::Text => {
                    println!("Stored Blobs:");
                    for digest in &blobs {
                        let size = host.blobs.size(digest).unwrap_or(0);
                        println!("  {} ({} bytes)", hex::encode(digest), size);
                    }
                    println!("\nTotal: {} blobs", blobs.len());
                }
                OutputFormat::Toml => {
                    let hex_digests: Vec<String> = blobs.iter().map(|d| hex::encode(d)).collect();
                    println!("{}", toml::to_string_pretty(&hex_digests)?);
                }
            }
            Ok(())
        }
    }
}

fn read_plan(path: &PathBuf) -> Result<compose_host_wasmtime::types::PlanV1> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read plan from {:?}", path))?;

    // Try JSON first
    match serde_json::from_str(&content) {
        Ok(plan) => return Ok(plan),
        Err(e) => eprintln!("JSON parse error: {}", e),
    }

    // Try TOML
    match toml::from_str(&content) {
        Ok(plan) => return Ok(plan),
        Err(e) => eprintln!("TOML parse error: {}", e),
    }

    anyhow::bail!("Failed to parse plan as JSON or TOML");
}

fn output<T: serde::Serialize>(format: &OutputFormat, message: &str, data: &T) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(data)?);
        }
        OutputFormat::Text => {
            println!("{}", message);
        }
        OutputFormat::Toml => {
            println!("{}", toml::to_string_pretty(data)?);
        }
    }
    Ok(())
}
