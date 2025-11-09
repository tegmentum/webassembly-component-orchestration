# composectl - WebAssembly Composition CLI

Command-line interface for composing, emitting, and executing WebAssembly component plans.

## Installation

```bash
cargo install --path tools/composectl
```

## Usage

```bash
composectl [OPTIONS] <COMMAND>
```

### Global Options

- `-f, --format <FORMAT>` - Output format (text, json, toml) [default: text]
- `-v, --verbose` - Enable verbose logging
- `-h, --help` - Print help

## Commands

### Plan Management

```bash
# Validate a plan file
composectl plan validate <plan.json>

# Show plan information  
composectl plan info <plan.json>
```

### Emit/Build

```bash
# Build/compose an artifact from a plan
composectl emit build <plan.json> --output <artifact.wasm>
```

### Execute

```bash
# Execute a plan as a CLI application
composectl exec run <plan.json> [args...]

# Invoke a specific export
composectl exec invoke <plan.json> <export-name>
```

### Secrets

```bash
# List available secrets
composectl secrets list [--backend <uri>]

# Resolve a secret by ID
composectl secrets resolve <id> --backend <uri>
```

### Trust & Verification

```bash
# Verify an artifact's signature
composectl trust verify <artifact> [--signature <sig-file>]

# List trusted artifacts
composectl trust list
```

### Reflection

```bash
# List exports from a composed component
composectl reflect exports <plan.json>

# Describe a specific export
composectl reflect describe <plan.json> <export-name>
```

### Metrics & Observability

```bash
# List collected metrics
composectl metrics list [--filter <pattern>] [--since <timestamp>]

# Get metric summary
composectl metrics summary <metric-name>
```

## Output Formats

### Text (default)
Human-readable output for terminal use.

### JSON
Machine-readable JSON output.

```bash
composectl --format json metrics list
```

### TOML
TOML format output.

```bash
composectl --format toml plan info plan.json
```

## Plan File Format

Plans can be in JSON or TOML format:

```json
{
  "version": "1",
  "root": "my-component",
  "components": [
    {
      "id": "my-component",
      "digest": [...],
      "source": null
    }
  ],
  "bindings": [],
  "secrets": [],
  "policy": {
    "determinism": "relaxed",
    "capabilities": [],
    "limits": {
      "max_memory_bytes": 10485760,
      "max_execution_time_ms": 5000
    }
  }
}
```

## Examples

### Validate and Execute

```bash
# Validate plan
composectl plan validate myapp.json

# Execute with arguments
composectl exec run myapp.json -- --help

# View metrics
composectl metrics list
```

### Build and Deploy

```bash
# Build artifact
composectl emit build myapp.json --output myapp.wasm

# Verify signature
composectl trust verify myapp.wasm --signature myapp.sig
```

## Environment Variables

- `RUST_LOG` - Set log level (error, warn, info, debug, trace)

## Implementation Status

✅ **Complete:**
- CLI argument parsing
- Plan validation
- Metrics listing and summary
- Output formatting (text, JSON, TOML)

🚧 **Partial:**
- Plan emission (stub implementation)
- Execution (basic implementation)
- Secrets resolution (concept/stub)
- Trust verification (concept/stub)
- Reflection (requires WIT introspection)

## Architecture

`composectl` is built on:
- **clap** - Command-line parsing
- **compose-host-wasmtime** - Reference host implementation
- **serde** - Serialization/deserialization
- **anyhow** - Error handling

## Development

```bash
# Build
cargo build --manifest-path tools/composectl/Cargo.toml

# Test
cargo test --manifest-path tools/composectl/Cargo.toml

# Run
cargo run --manifest-path tools/composectl/Cargo.toml -- --help
```

## See Also

- Examples in `examples/`
- Host implementation in `hosts/wasmtime/`
- WIT interfaces in `wit/`
