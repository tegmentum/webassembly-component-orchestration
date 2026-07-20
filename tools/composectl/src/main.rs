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

    // Initialize host. composectl is a build-time tool operating on
    // trusted local files, so it uses the 1 GiB `build_tool()`
    // ceiling rather than the multi-tenant 100 MiB default. A
    // `--max-blob-size` flag (or `COMPOSECTL_MAX_BLOB_SIZE` env var)
    // can raise or lower this per invocation.
    let mut config = HostConfig::build_tool();
    if let Some(max) = cli.max_blob_size {
        config.max_blob_size = max;
    }
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

            // Validate the plan file's structure. Blob availability is an
            // emit/exec concern, so a standalone plan file is linted without
            // requiring its component artifacts to be staged locally.
            let validator = host.plan_validator();
            validator.validate_structure(&plan_data)?;

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

fn handle_emit(host: &CompositorHost, action: EmitAction, _format: &OutputFormat) -> Result<()> {
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

fn handle_exec(host: &CompositorHost, action: ExecAction, _format: &OutputFormat) -> Result<()> {
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

            // Invoke is the CBOR-marshalled invocation path: the caller
            // hands `ExecHandler::invoke` a CBOR array of arguments and
            // gets back a CBOR array of results (same wire format as
            // `compose:host/invoker.call-with-cbor`). We don't yet expose
            // an arg-passing flag on the CLI, so we send a canonical
            // empty CBOR array — exports with zero params invoke, exports
            // with params surface an "expected N argument(s), got 0"
            // error to the user.
            const EMPTY_CBOR_ARRAY: &[u8] = &[0x80];

            let exec_handler = host.exec_handler();
            let result = exec_handler.invoke(&plan_data, &export, EMPTY_CBOR_ARRAY)?;

            println!(
                "Invoked export '{}' ({} result bytes)",
                export,
                result.len()
            );
            println!("Result (CBOR, hex): {}", hex::encode(&result));
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
            let secrets = host
                .secrets
                .list_secrets(backend.as_ref())
                .map_err(|e| anyhow::anyhow!("failed to list secrets: {}", e))?;

            let label = backend.as_deref().unwrap_or("all backends");
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&secrets)?);
                }
                OutputFormat::Text => {
                    println!("Secrets in backend '{}':", label);
                    for id in &secrets {
                        println!("  {}", id);
                    }
                    println!("\nTotal: {} secrets", secrets.len());
                }
                OutputFormat::Toml => {
                    println!("{}", toml::to_string_pretty(&secrets)?);
                }
            }
            Ok(())
        }
        SecretsAction::Resolve { id, backend } => {
            // Resolving mints an opaque bearer token — the token, not the
            // plaintext secret, is what plans reference. Printing plaintext
            // here would defeat the point.
            let token = host
                .secrets
                .resolve(&id, &backend)
                .map_err(|e| anyhow::anyhow!("failed to resolve secret: {}", e))?;

            match format {
                OutputFormat::Json => {
                    let payload = serde_json::json!({
                        "id": id,
                        "backend": backend,
                        "token": token,
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                }
                OutputFormat::Text => {
                    println!("Resolved secret '{}' from backend '{}'", id, backend);
                    println!("  Token: {}", token);
                }
                OutputFormat::Toml => {
                    let payload = toml::Table::from_iter([
                        ("id".to_string(), toml::Value::String(id.clone())),
                        ("backend".to_string(), toml::Value::String(backend.clone())),
                        ("token".to_string(), toml::Value::String(token.clone())),
                    ]);
                    println!("{}", toml::to_string_pretty(&payload)?);
                }
            }
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
            use sha2::{Digest as _, Sha256};

            let bytes = fs::read(&artifact)
                .with_context(|| format!("Failed to read artifact {:?}", artifact))?;
            let digest = Sha256::digest(&bytes).to_vec();

            let signature_bytes = signature
                .as_ref()
                .map(|p| fs::read(p).with_context(|| format!("Failed to read signature {:?}", p)))
                .transpose()?;

            let result = host
                .trust
                .verify(&digest, &bytes, signature_bytes.as_deref())
                .map_err(|e| anyhow::anyhow!("verification failed: {}", e))?;

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                OutputFormat::Text => {
                    println!("Artifact: {:?}", artifact);
                    println!("  Digest: {}", hex::encode(&digest));
                    println!("  Verified: {}", result.verified);
                    println!("  Signer: {}", result.metadata.signer);
                    println!("  Backend: {}", result.metadata.backend);
                    if let Some(ts) = result.metadata.timestamp {
                        println!("  Timestamp: {}", ts);
                    }
                }
                OutputFormat::Toml => {
                    println!("{}", toml::to_string_pretty(&result)?);
                }
            }
            Ok(())
        }
        TrustAction::List => {
            // `list_trusted` returns each digest with the verification
            // metadata recorded at trust time (HashMap-backed, so order is
            // not stable — sort by digest so the CLI output is deterministic).
            let mut entries = host.trust.list_trusted();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));

            #[derive(serde::Serialize)]
            struct TrustedEntry<'a> {
                digest: String,
                metadata: &'a compose_host_wasmtime::types::VerificationMetadata,
            }
            let view: Vec<TrustedEntry<'_>> = entries
                .iter()
                .map(|(d, m)| TrustedEntry {
                    digest: hex::encode(d),
                    metadata: m,
                })
                .collect();

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&view)?);
                }
                OutputFormat::Text => {
                    println!("Trusted artifacts:");
                    for entry in &view {
                        println!(
                            "  {}  (signer={}, backend={})",
                            entry.digest, entry.metadata.signer, entry.metadata.backend,
                        );
                    }
                    println!("\nTotal: {} trusted artifacts", view.len());
                }
                OutputFormat::Toml => {
                    println!("{}", toml::to_string_pretty(&view)?);
                }
            }
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
            let exec_handler = host.exec_handler();
            let exports = exec_handler.list_exports(&plan_data)?;

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&exports)?);
                }
                OutputFormat::Text => {
                    println!("Exports from plan:");
                    for name in &exports {
                        println!("  {}", name);
                    }
                    println!("\nTotal: {} exports", exports.len());
                }
                OutputFormat::Toml => {
                    println!("{}", toml::to_string_pretty(&exports)?);
                }
            }
            Ok(())
        }
        ReflectAction::Describe { plan, export } => {
            let plan_data = read_plan(&plan)?;
            let exec_handler = host.exec_handler();
            let info = exec_handler.describe_export(&plan_data, &export)?;

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&info)?);
                }
                OutputFormat::Text => {
                    println!("Export: {}", info.name);
                    println!("  Signature: {}", info.type_sig);
                }
                OutputFormat::Toml => {
                    println!("{}", toml::to_string_pretty(&info)?);
                }
            }
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
        MetricsAction::List { filter, since: _ } => {
            // List all metrics, filtering handled separately if needed
            let metrics = host.metrics.list(filter.as_deref(), None, None);

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&metrics)?);
                }
                OutputFormat::Text => {
                    println!("Collected Metrics:");
                    for metric in &metrics {
                        println!(
                            "  {} = {:?} @ {}",
                            metric.name, metric.value, metric.timestamp
                        );
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
            let bytes =
                fs::read(&file).with_context(|| format!("Failed to read file {:?}", file))?;

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
            let digest_bytes =
                hex::decode(&digest).with_context(|| format!("Invalid hex digest: {}", digest))?;

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
            let digest_bytes =
                hex::decode(&digest).with_context(|| format!("Invalid hex digest: {}", digest))?;

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
                    let hex_digests: Vec<String> = blobs.iter().map(hex::encode).collect();
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
                    let hex_digests: Vec<String> = blobs.iter().map(hex::encode).collect();
                    println!("{}", toml::to_string_pretty(&hex_digests)?);
                }
            }
            Ok(())
        }
    }
}

fn read_plan(path: &PathBuf) -> Result<compose_host_wasmtime::types::PlanV1> {
    let bytes = fs::read(path).with_context(|| format!("Failed to read plan from {:?}", path))?;

    // The canonical plan encoding is CBOR (e.g. the conformance vectors and
    // the digest preimage). Decode `.cbor` with the canonical deserializer;
    // otherwise treat the file as UTF-8 text and try JSON then TOML.
    let is_cbor = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cbor"));
    if is_cbor {
        return compose_host_wasmtime::plan::deserialize(&bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse plan as CBOR: {}", e.message));
    }

    let content = String::from_utf8(bytes)
        .context("plan is not valid UTF-8; for a binary CBOR plan use a .cbor file extension")?;
    match serde_json::from_str(&content) {
        Ok(plan) => return Ok(plan),
        Err(e) => eprintln!("JSON parse error: {}", e),
    }
    match toml::from_str(&content) {
        Ok(plan) => return Ok(plan),
        Err(e) => eprintln!("TOML parse error: {}", e),
    }
    anyhow::bail!("Failed to parse plan as JSON, TOML, or CBOR");
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
