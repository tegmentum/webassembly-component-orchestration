use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

use anyhow::{anyhow, Context, Result};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(
    name = "conformance-runner",
    about = "Run canonical fixture checks against composectl"
)]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// List fixtures registered with the runner.
    List,
    /// Execute fixtures and assert their expected outcomes.
    Run {
        /// Optional substring filter on fixture names.
        #[arg(long)]
        fixture: Option<String>,
        /// Optional path to write a JSON summary (array of FixtureResult).
        #[arg(long)]
        json: Option<Utf8PathBuf>,
    },
}

#[derive(Debug, Serialize)]
struct FixtureResult {
    name: String,
    path: Utf8PathBuf,
    expect_pass: bool,
    actual_pass: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Fixture {
    name: String,
    path: Utf8PathBuf,
    expect_pass: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let fixtures = load_fixtures()?;

    match cli.command {
        CliCommand::List => list_fixtures(&fixtures),
        CliCommand::Run { fixture, json } => {
            run_fixtures(&fixtures, fixture.as_deref(), json.as_ref())
        }
    }
}

fn list_fixtures(fixtures: &[Fixture]) -> Result<()> {
    for fixture in fixtures {
        println!(
            "{}\t{}\t{}",
            fixture.name,
            fixture.path,
            if fixture.expect_pass { "pass" } else { "fail" }
        );
    }
    Ok(())
}

fn run_fixtures(
    fixtures: &[Fixture],
    filter: Option<&str>,
    json_output: Option<&Utf8PathBuf>,
) -> Result<()> {
    let composectl = locate_composectl()?;
    let mut failures = Vec::new();
    let mut summary = Vec::new();

    for fixture in fixtures.iter().filter(|fx| {
        filter
            .map(|needle| fx.name.contains(needle))
            .unwrap_or(true)
    }) {
        let execution = run_fixture(&composectl, fixture)?;
        let expected = fixture.expect_pass;
        println!(
            "fixture={} expected={} actual={}",
            fixture.name, expected, execution.success
        );
        if execution.success != expected {
            failures.push(fixture.name.clone());
        }
        summary.push(FixtureResult {
            name: fixture.name.clone(),
            path: fixture.path.clone(),
            expect_pass: expected,
            actual_pass: execution.success,
            stdout: execution.stdout,
            stderr: execution.stderr,
        });
    }

    if let Some(path) = json_output {
        let json = serde_json::to_string_pretty(&summary)
            .context("failed to serialize fixture results")?;
        fs::write(path, json)
            .with_context(|| format!("failed to write JSON summary to {}", path))?;
    }

    if !failures.is_empty() {
        return Err(anyhow!(
            "fixtures failed validation: {}",
            failures.join(", ")
        ));
    }

    Ok(())
}

struct ExecutionOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn run_fixture(composectl: &Path, fixture: &Fixture) -> Result<ExecutionOutput> {
    let output = StdCommand::new(composectl)
        .arg("plan")
        .arg("validate")
        .arg(fixture.path.as_std_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to execute composectl for {}", fixture.name))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(ExecutionOutput {
        success: output.status.success(),
        stdout,
        stderr,
    })
}

fn locate_composectl() -> Result<PathBuf> {
    if let Some(bin) = option_env!("CARGO_BIN_EXE_composectl") {
        return Ok(PathBuf::from(bin));
    }

    let runner_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = runner_dir
        .parent()
        .and_then(|p| p.parent())
        .context("failed to determine workspace root from runner directory")?
        .to_owned();

    let candidates = [
        workspace_root.join("target").join("debug").join(bin_name()),
        workspace_root
            .join("target")
            .join("release")
            .join(bin_name()),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.as_std_path().to_path_buf());
        }
    }

    Err(anyhow!(
        "unable to locate composectl binary; build the workspace or install composectl in PATH"
    ))
}

fn bin_name() -> &'static str {
    if cfg!(windows) {
        "composectl.exe"
    } else {
        "composectl"
    }
}

fn load_fixtures() -> Result<Vec<Fixture>> {
    let runner_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = runner_dir
        .parent()
        .and_then(|p| p.parent())
        .context("failed to determine workspace root from runner directory")?
        .to_owned();

    let vectors_root = workspace_root.join("conformance").join("vectors");
    let manifest_json = vectors_root.join("fixtures.json");
    let bytes = fs::read(manifest_json.as_std_path()).with_context(|| {
        format!(
            "failed to read conformance fixture manifest at {}",
            manifest_json
        )
    })?;

    let mut fixtures: Vec<Fixture> =
        serde_json::from_slice(&bytes).context("failed to parse fixtures.json")?;
    for fixture in &mut fixtures {
        fixture.path = workspace_root.join(&fixture.path);
    }
    Ok(fixtures)
}
