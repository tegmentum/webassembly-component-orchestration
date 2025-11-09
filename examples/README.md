# Examples & Demos

Working examples and demonstrations of the WebAssembly Compositional System.

## Quick Start

```bash
# Build all examples
./build-all.sh

# Test all examples and demos
./test-all.sh
```

## Examples

### hello-cli

Simple command-line component demonstrating basic composition and execution.

**Features:**
- CLI argument parsing
- Standard output and error streams
- Exit codes
- Metrics collection via stderr logging

**Build & Run:**
```bash
cd hello-cli
./build.sh
./run.sh "YourName"
```

**Output:**
```
[INFO] Starting hello-cli
[DEBUG] Arguments received: 1
Hello, YourName!
Welcome to the WebAssembly Compositional System!
[INFO] Greeting complete
[METRIC] execution_time_ms: 1
[METRIC] output_bytes: 58
```

### hello-http

HTTP server component demonstrating WASI HTTP capabilities.

**Features:**
- HTTP request handling with `wasi:http/incoming-handler`
- Multiple endpoints (/, /hello, /health)
- JSON and text responses
- Structured logging (INFO, DEBUG, METRIC)
- Error handling (404 responses)
- Pure WebAssembly component (105KB)

**Build & Run:**
```bash
cd hello-http
./build.sh
./run.sh

# In another terminal:
curl http://localhost:8080/
curl http://localhost:8080/hello
curl http://localhost:8080/health
```

**Note:** Requires wasmtime with HTTP support or a compositor host with HTTP capabilities. See `hello-http/README.md` for full details.

## Demonstrations

Interactive demos showing key system features. Located in `demos/`.

### Secrets Demo (`demos/secrets-demo.sh`)

Shows secret injection patterns:
- Environment variable secrets
- Vault integration (simulated)
- PKCS#11 key access (simulated)

```bash
cd demos
./secrets-demo.sh
```

### Trust Demo (`demos/trust-demo.sh`)

Shows signature verification workflow:
- Unsigned component rejection
- Signed component acceptance  
- Multiple trust backends (SigStore, PGP, X.509)

```bash
cd demos
./trust-demo.sh
```

### Determinism Demo (`demos/determinism-demo.sh`)

Shows execution modes:
- Strict determinism (reproducible)
- Relaxed determinism (with syscalls)
- Policy enforcement

```bash
cd demos
./determinism-demo.sh
```

### Run All Demos

```bash
cd demos
./run-all-demos.sh
```

## Architecture

### Example Structure

Each example follows this pattern:

```
example-name/
├── src/main.rs       # Rust source
├── Cargo.toml        # Package manifest
├── build.sh          # Build script
├── run.sh            # Run script
└── README.md         # Documentation
```

### Demo Structure

Demos are shell scripts that illustrate concepts through:
- Example plans and configurations
- Simulated execution flows
- Policy enforcement examples
- Expected behavior output

## Building from Source

### Prerequisites

- Rust toolchain (install from https://rustup.rs/)
- `wasm32-wasip1` target
- wasmtime runtime (install from https://wasmtime.dev/)

### Build Process

```bash
# Add WASM target if not installed
rustup target add wasm32-wasip1

# Build individual example
cd hello-cli
cargo build --release --target wasm32-wasip1

# Or build all examples
cd ..
./build-all.sh
```

### Testing

```bash
# Test all examples and demos
./test-all.sh

# Test individual example
cd hello-cli
./run.sh "TestUser"
```

## Observability

All examples demonstrate observability features:

**Logs** (to stderr):
```
[INFO] Starting hello-cli
[DEBUG] Arguments received: 1  
[ERROR] Something went wrong
```

**Metrics** (to stderr):
```
[METRIC] execution_time_ms: 42
[METRIC] output_bytes: 1234
```

**Audit** (via host):
- Component execution events
- Policy enforcement decisions
- Secret access attempts

## Implementation References

Examples demonstrate concepts implemented in:
- **Execution**: `hosts/wasmtime/src/exec.rs`
- **Secrets**: `hosts/wasmtime/src/secrets/`
- **Trust**: `hosts/wasmtime/src/trust/`
- **Metrics**: `hosts/wasmtime/src/metrics.rs`
- **Audit**: `hosts/wasmtime/src/audit/`

## Acceptance Criteria (M10)

- ✅ hello-cli builds and runs
- ✅ hello-http implemented and builds (105KB component)
- ✅ Secrets demo created and tested
- ✅ Trust demo created and tested
- ✅ Determinism demo created and tested
- ✅ All examples generate logs and metrics
- ✅ Test suite passes (hello-cli + all demos)

## Next Steps

- Expand hello-http with WASI HTTP when available
- Add more complex composition examples
- Demonstrate multi-component workflows
- Show distributed execution patterns

## Troubleshooting

**wasmtime not found:**
```bash
# Install wasmtime
curl https://wasmtime.dev/install.sh -sSf | bash
```

**wasm32-wasip1 target missing:**
```bash
rustup target add wasm32-wasip1
```

**Build warnings about workspace profiles:**
These are harmless. The examples are part of the workspace but use their own build settings.
