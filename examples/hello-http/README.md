# Hello HTTP Example

HTTP server component demonstrating WASI HTTP capabilities and request/response handling.

## Features

- **HTTP Request Handling**: Exports `wasi:http/incoming-handler`
- **Multiple Endpoints**: Root, hello, and health check
- **Content Types**: JSON and text responses
- **Logging**: Structured logging (INFO, DEBUG, METRIC)
- **Error Handling**: Proper 404 responses
- **Component Architecture**: Pure WebAssembly component (187KB optimized)

## Building

```bash
./build.sh
```

This will:
1. Install wit-deps (if needed)
2. Fetch WASI HTTP dependencies
3. Build the WebAssembly component

Output: `target/wasm32-wasip2/release/hello_http.wasm`

## Running

```bash
./run.sh
```

The server starts on http://localhost:8080 (if wasmtime supports HTTP serving)

## Endpoints

- `GET /` - Plain text hello message
- `GET /hello` - JSON response with version info
- `GET /health` - Health check endpoint

## Testing

```bash
# Start the server (in one terminal)
./run.sh

# Test endpoints (in another terminal)
curl http://localhost:8080/
curl http://localhost:8080/hello
curl http://localhost:8080/health

# Test 404
curl http://localhost:8080/notfound
```

## Architecture

The hello-http example consists of:

1. **HTTP Handler Component** (`src/lib.rs`)
   - Implements `wasi:http/incoming-handler`
   - Routes requests to different endpoints
   - Generates appropriate responses

2. **WIT Definition** (`wit/world.wit`)
   - Defines the component world
   - Exports HTTP handler interface

3. **Composition Plan** (in `run.sh`)
   - Defines execution policy
   - Sets resource limits
   - Configures network access

## Observability

The component emits structured logs:

```
[INFO] Received HTTP request
[DEBUG] Method: Get, Path: /hello
[METRIC] response_status: 200
[METRIC] response_bytes: 52
[INFO] Request handled successfully
```

These can be collected by the host for monitoring and debugging.

## Requirements

- Rust toolchain with wasm32-wasip2 target
- wit-deps CLI tool
- wasmtime with HTTP support (for running)

## Implementation Details

This example demonstrates:
- Pure WebAssembly component (no platform-specific code)
- Standard WASI HTTP interfaces
- Clean separation of routing and response logic
- Structured logging for observability
- Error handling patterns

## Next Steps

For production use, consider:
- Authentication and authorization
- Rate limiting
- Request validation
- Database integration (via composed components)
- Metrics collection
- TLS/HTTPS support
