/// CLI command structure and argument parsing
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "composectl",
    version,
    about = "Command-line interface for the compositional WebAssembly system",
    long_about = None,
)]
pub struct Cli {
    /// Output format
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Text, global = true)]
    pub format: OutputFormat,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Maximum blob size in bytes accepted by the blob CAS. Overrides
    /// the default 1 GiB build-tool ceiling. Also honours
    /// `COMPOSECTL_MAX_BLOB_SIZE` from the environment when the flag
    /// is not set. Use a larger value to admit composed runtimes above
    /// the default (e.g. postgis-composed.wasm).
    #[arg(
        long,
        global = true,
        env = "COMPOSECTL_MAX_BLOB_SIZE",
        value_name = "BYTES"
    )]
    pub max_blob_size: Option<u64>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum OutputFormat {
    /// Plain text output
    Text,
    /// JSON output
    Json,
    /// TOML output
    Toml,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Plan management operations
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },

    /// Emit/build composed artifacts
    Emit {
        #[command(subcommand)]
        action: EmitAction,
    },

    /// Execute composed artifacts
    Exec {
        #[command(subcommand)]
        action: ExecAction,
    },

    /// Secret management
    Secrets {
        #[command(subcommand)]
        action: SecretsAction,
    },

    /// Trust and verification
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },

    /// Component reflection
    Reflect {
        #[command(subcommand)]
        action: ReflectAction,
    },

    /// Metrics and observability
    Metrics {
        #[command(subcommand)]
        action: MetricsAction,
    },

    /// Blob storage management
    Blob {
        #[command(subcommand)]
        action: BlobAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum PlanAction {
    /// Validate a plan file
    Validate {
        /// Path to the plan file
        plan: PathBuf,
    },

    /// Show plan information
    Info {
        /// Path to the plan file
        plan: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum EmitAction {
    /// Build/compose an artifact from a plan
    Build {
        /// Path to the plan file
        plan: PathBuf,

        /// Output path for the composed artifact
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum ExecAction {
    /// Execute a plan as a CLI application
    Run {
        /// Path to the plan file
        plan: PathBuf,

        /// Arguments to pass to the application
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Invoke a specific export from a composed component
    Invoke {
        /// Path to the plan file
        plan: PathBuf,

        /// Name of the export to invoke
        export: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SecretsAction {
    /// List available secrets
    List {
        /// Backend URI (e.g., dev://, pkcs11://)
        #[arg(long)]
        backend: Option<String>,
    },

    /// Resolve a secret by ID
    Resolve {
        /// Secret ID
        id: String,

        /// Backend URI
        #[arg(long)]
        backend: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TrustAction {
    /// Verify an artifact's signature
    Verify {
        /// Path to the artifact
        artifact: PathBuf,

        /// Path to the signature file
        #[arg(long)]
        signature: Option<PathBuf>,
    },

    /// List trusted artifacts
    List,
}

#[derive(Subcommand, Debug)]
pub enum ReflectAction {
    /// List exports from a composed component
    Exports {
        /// Path to the plan file
        plan: PathBuf,
    },

    /// Describe a specific export
    Describe {
        /// Path to the plan file
        plan: PathBuf,

        /// Name of the export to describe
        export: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MetricsAction {
    /// List collected metrics
    List {
        /// Filter metrics by name pattern
        #[arg(long)]
        filter: Option<String>,

        /// Show metrics since timestamp (milliseconds)
        #[arg(long)]
        since: Option<u64>,
    },

    /// Get metric summary
    Summary {
        /// Metric name
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum BlobAction {
    /// Store a blob (component) from a file
    Put {
        /// Path to the file to store
        file: PathBuf,
    },

    /// Retrieve a blob by digest
    Get {
        /// Digest (hex-encoded SHA-256)
        digest: String,

        /// Output path
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Check if a blob exists
    Has {
        /// Digest (hex-encoded SHA-256)
        digest: String,
    },

    /// List all blobs in storage
    List,
}
