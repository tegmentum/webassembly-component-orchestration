# Component Composition Integration Guide

## Overview

The WebAssembly component composition system now features **full static composition** using an in-memory bytes-based API. This document explains how the composition flow works and how to use it via the `composectl` CLI tool.

## Architecture

```
┌─────────────┐
│ composectl  │  CLI tool for composition operations
└──────┬──────┘
       │
       ├─────────────────────────────────────┐
       │                                     │
       ▼                                     ▼
┌──────────────┐                    ┌────────────────┐
│ CompositorHost│                    │  BlobStore     │
│               │◄───────────────────┤  (CAS)         │
│ - EmitHandler │                    │                │
│ - ExecHandler │                    │  SHA-256 based │
│ - Validators  │                    │  storage       │
└──────┬────────┘                    └────────────────┘
       │
       ▼
┌─────────────────────────────────────────────────────┐
│         Bytes-Based Static Composition              │
│                                                     │
│  ┌──────────────┐      ┌────────────────────────┐ │
│  │ Root Component│      │ Dependency Components │ │
│  │   (bytes)     │      │      (bytes)          │ │
│  └───────┬───────┘      └──────────┬─────────────┘ │
│          │                         │               │
│          └────────┬────────────────┘               │
│                   ▼                                │
│        ┌─────────────────────────┐                │
│        │ BytesComponentComposer  │                │
│        │  (wasm-compose fork)    │                │
│        └──────────┬──────────────┘                │
│                   ▼                                │
│        ┌─────────────────────────┐                │
│        │  Composed Component     │                │
│        │  (single .wasm file)    │                │
│        └─────────────────────────┘                │
└─────────────────────────────────────────────────────┘
```

## Key Features

### 1. In-Memory Composition
- **No file I/O**: All composition happens in memory using `Cow<[u8]>`
- **Zero-copy**: Efficient memory usage with copy-on-write semantics
- **Blob store integration**: Works seamlessly with content-addressed storage

### 2. Full Static Composition
- **Wasm-compose powered**: Uses official Bytecode Alliance composition logic
- **Correct by construction**: Handles all Component Model complexities
- **Proper linking**: Wires imports/exports according to bindings

### 3. Caching & Performance
- **Emit-key based caching**: Identical plans reuse cached results
- **Content-addressed**: SHA-256 digests for deduplication
- **Validation**: All components validated before composition

## Using composectl

### Basic Workflow

#### 1. Store Components in Blob Store

```bash
# Components are stored by their content hash (SHA-256)
composectl blob put component1.wasm
# Output: Stored with digest: a1b2c3d4...

composectl blob put component2.wasm
# Output: Stored with digest: e5f6g7h8...
```

#### 2. Create a Composition Plan

Create a plan file (CBOR format) that specifies:
- Root component
- Dependencies
- Bindings (import/export wiring)
- Policy constraints

Example plan structure:
```json
{
  "version": "1",
  "root": "my-app",
  "components": [
    {
      "id": "my-app",
      "digest": "a1b2c3d4...",
      "source": "https://registry.example.com/my-app"
    },
    {
      "id": "logger",
      "digest": "e5f6g7h8...",
      "source": "https://registry.example.com/logger"
    }
  ],
  "bindings": [
    {
      "import_name": "logging:api/logger",
      "provider_id": "logger",
      "export_name": "logging:api/logger"
    }
  ],
  "secrets": [],
  "policy": {
    "determinism": "relaxed",
    "capabilities": [],
    "limits": {}
  }
}
```

#### 3. Validate the Plan

```bash
composectl plan validate myplan.cbor
# Output: Plan is valid
```

#### 4. Compose the Artifact

```bash
composectl emit build myplan.cbor --output composed.wasm
```

This command:
1. Loads the plan from `myplan.cbor`
2. Retrieves all component bytes from the blob store
3. Validates each component
4. Performs static composition using BytesComponentComposer
5. Stores the result in the blob store
6. Writes the composed component to `composed.wasm`

**Output:**
```
INFO  performing static composition dependencies: 1
INFO  added dependency to composition id: logger, size: 12345 bytes
INFO  static composition complete output size: 23456 bytes
Artifact written to: "composed.wasm"
Digest: a1b2c3d4e5f6g7h8...
```

#### 5. Execute the Composed Component

```bash
composectl exec run myplan.cbor -- arg1 arg2
# Executes the composed component with arguments
```

## Implementation Details

### Composition Flow

1. **Plan Loading** (`handle_emit` in main.rs)
   - Reads plan file (CBOR format)
   - Parses into `PlanV1` structure

2. **Component Loading** (`compose_internal` in emit.rs)
   - Retrieves all component bytes from blob store
   - Validates each component using wasmparser
   - Builds component map: `HashMap<ComponentId, Vec<u8>>`

3. **Static Composition** (`compose_with_wrapper` in emit.rs)
   - Creates `BytesConfig` with dependencies
   - Instantiates `BytesComponentComposer`
   - Calls `.compose()` for full static linking
   - Returns composed WebAssembly bytes

4. **Result Storage**
   - Computes digest of composed artifact
   - Stores in blob store
   - Updates emit-key cache
   - Writes to output file

### Code Structure

```
tools/composectl/src/
├── main.rs           # CLI entry point and command handlers
└── cli.rs            # Clap argument definitions

hosts/wasmtime/src/
├── emit.rs           # EmitHandler with static composition
├── blobs.rs          # Content-addressed storage
├── plan.rs           # Plan validation
├── exec.rs           # Execution runtime
└── types.rs          # Common types and structures
```

### Key Functions

**EmitHandler::compose()**
```rust
pub fn compose(&self, plan: &PlanV1) -> Result<CompositionResult, Error>
```
- Main entry point for composition
- Validates plan
- Checks cache
- Delegates to `compose_internal`
- Updates cache on success

**compose_with_wrapper()**
```rust
fn compose_with_wrapper(
    &self,
    plan: &PlanV1,
    component_map: &HashMap<String, Vec<u8>>,
    root_bytes: &[u8],
) -> Result<Vec<u8>, Error>
```
- Builds `BytesConfig` from bindings
- Creates `BytesComponentComposer`
- Performs static composition
- Returns composed bytes

## Modified wasm-tools Fork

The implementation uses a forked version of wasm-tools with bytes-based APIs:

**Repository**: https://github.com/tegmentum/wasm-tools/tree/bytes-api

**Key Additions**:
- `BytesConfig` - Configuration from in-memory bytes
- `BytesDependency` - Component dependency as bytes
- `BytesComponentComposer` - Composer without file I/O

**Dependency Configuration** (Cargo.toml):
```toml
wasm-tools = { git = "https://github.com/tegmentum/wasm-tools.git", branch = "bytes-api" }
wasm-compose = { git = "https://github.com/tegmentum/wasm-tools.git", branch = "bytes-api" }
wit-component = { git = "https://github.com/tegmentum/wasm-tools.git", branch = "bytes-api" }
wasmparser = { git = "https://github.com/tegmentum/wasm-tools.git", branch = "bytes-api" }
wasm-encoder = { git = "https://github.com/tegmentum/wasm-tools.git", branch = "bytes-api" }
```

## Testing

Run the test suite:
```bash
# Test composition logic
cargo test -p compose-host-wasmtime emit::tests

# All tests should pass:
# - test_emit_key_computation
# - test_single_component_composition
# - test_composition_with_missing_blob
# - test_composition_cache
# - test_composition_validates_bindings
# - test_validate_component
# - test_get_artifact
```

## Performance Characteristics

| Operation | Performance | Notes |
|-----------|-------------|-------|
| Component loading | O(n) where n = components | Parallel blob retrieval possible |
| Validation | O(n) per component | Cached after first validation |
| Composition | O(n*m) where m = bindings | wasm-compose internal complexity |
| Caching | O(1) lookup | Emit-key based, SHA-256 hash |
| Total for cached | O(1) | Cache hit returns immediately |

## Comparison with Previous Approach

| Aspect | Runtime Linking (Old) | Static Composition (New) |
|--------|----------------------|--------------------------|
| **Composition** | Deferred to runtime | Done at build time |
| **Output** | Root component only | Fully linked component |
| **Runtime deps** | Wasmtime resolves imports | Self-contained artifact |
| **Portability** | Requires all components | Single file deployment |
| **Performance** | Import resolution overhead | Optimized static linking |
| **Correctness** | Runtime errors possible | Validated at build time |

## Turtle output

For hosts that want to expose a composition plan through a graph store
(e.g. the Stardog `wf:compose` plugin), the orchestrator's WIT surface
now includes a Turtle-serialization export:

```wit
package sys:compose@1.0.0;

interface rdf {
  use types.{error};

  plan-to-turtle: func(plan-cbor: list<u8>) -> result<string, error>;

  plan-to-turtle-with-iri: func(plan-cbor: list<u8>, plan-iri: string)
    -> result<string, error>;

  plan-to-turtle-with-artifact: func(
      plan-cbor: list<u8>,
      plan-iri: string,
      artifact-url: string,
      digest-hex: option<string>,
  ) -> result<string, error>;
}
```

Both `plan-to-turtle` and `plan-to-turtle-with-iri` take the canonical
CBOR bytes produced by `sys:compose/plan.serialize` and return a UTF-8
Turtle document. The default plan IRI is `urn:composition:plan`; call
`plan-to-turtle-with-iri` when you want a stable, plan-digest- or
tenant-namespaced subject.

`plan-to-turtle-with-artifact` additionally emits two composed-artifact
anchor triples on top of the standard plan RDF:

- `<plan-iri> comp:hasArtifact <artifact-url>` — REQUIRED; the URL the
  composed wasm is served from. Any URL scheme is valid — the plugin's
  default is `sha256://<hex>` (its in-tree content-addressed blob
  store), but operators re-hosting the composed artifact to
  `https://cdn.example.com/…` or `ipfs://Qm…` simply SPARQL-UPDATE the
  triple.
- `<plan-iri> comp:compositionDigest "<hex>"` — OPTIONAL; emitted only
  when `digest-hex` is `some(...)`. SHA-256 of the composed bytes as a
  lowercase-hex `xsd:string`. Stable across URL re-hosting — the
  content-identity anchor.

The reader (`plan-from-turtle`, `plan-from-turtle-with-iri`) silently
ignores both anchor predicates so plans carrying them still round-trip.
Downstream admins can SPARQL-join composition RDF directly against
extension-grant triples via the `comp:hasArtifact` object without
maintaining a side registry.

### Vocabulary

The Turtle document uses the composition vocabulary at
`http://tegmentum.ai/ns/composition/` (prefix `comp:`). Predicates
mirror the `PlanV1` fields one-for-one:

| Predicate | Domain | Range |
|-----------|--------|-------|
| `comp:version` | plan | string literal |
| `comp:root` | plan | component-id literal |
| `comp:component` | plan | blank node (Component) |
| `comp:binding` | plan | blank node (ImportBinding) |
| `comp:secret` | plan | blank node (SecretBinding) |
| `comp:policy` | plan | blank node (Policy) |
| `comp:linkage` | plan | `"static"` \| `"runtime"` |
| `comp:id` | Component | string literal |
| `comp:digest` | Component | `"sha256:<hex>"` literal |
| `comp:source` | Component | source URL literal |
| `comp:import` / `comp:provider` / `comp:export` / `comp:consumer` | ImportBinding | string literals |
| `comp:explicitExport` | plan | blank node (ExplicitExport) |
| `comp:sourceInstance` / `comp:interfaceName` | ExplicitExport | string literals |
| `comp:hasArtifact` | plan | artifact URL (IRI) — emitted by `plan-to-turtle-with-artifact` |
| `comp:compositionDigest` | plan | composed-bytes SHA-256 hex literal — optional; emitted by `plan-to-turtle-with-artifact` |

The full round-trip (writer + reader) lives in `libs/compose-rdf`;
see `plan_to_rdf`, `plan_to_turtle`, and `plan_from_rdf`. Both
`explicit_exports` and `policy.determinism` (when defaulted) are
lossless: the reader parses `comp:explicitExport` blank nodes back
into `PlanV1.explicit_exports` in insertion order, and the reader's
absent-`comp:determinism` fallback tracks `Policy::default()` in
compose-core so writer and reader defaults never drift.

### Consumer flow (Stardog example)

1. Client code fires a SPARQL query with `wf:compose(<plan-graph>, …)`.
2. The plugin loads the plan graph, hands it to the orchestrator, and
   the orchestrator returns the composed `.wasm` bytes plus a Turtle
   document (via `plan-to-turtle`).
3. The plugin inserts the Turtle into a named graph so downstream
   queries can inspect what was composed. Because the writer and
   reader are round-trip lossless, a later `wf:compose` invocation can
   consume the same graph unchanged.

## Host stub components

The compose-orchestrator wasm declares imports that model the handoff
from the native embedder. In a real deployment those imports are
satisfied by adapter components the embedder ships (a wasmtime adapter
for `tegmentum:runtime`; a startup wrapper for `host:bootstrap`; a
probe for `compose:host/runtime-info`).

For plugin embedders that only need the composed orchestrator to
answer `sys:compose/*` calls -- notably the Stardog / webassembly4j
path this repo is currently oriented around -- writing those adapters
in the host is disproportionate work. Instead, `libs/host-stubs/`
ships three trivial static-return components that satisfy each
functional import at `wac plug` time:

| Import                                    | Satisfied by         | Behaviour                                             |
| ----------------------------------------- | -------------------- | ----------------------------------------------------- |
| `compose:host/runtime-info@0.1.0`         | `runtime-info-stub`  | Returns fixed fingerprint (`stardog` / `1.0.0` / empty features / `jvm`). |
| `host:bootstrap/bootstrap@0.1.0`          | `bootstrap-stub`     | Returns empty args list.                              |
| `tegmentum:runtime/control@0.1.0`         | `runtime-control-stub` | No-op resource constructor; never actually invoked by the orchestrator today. |

`libs/compose-orchestrator-wasm/build.sh` builds all three stubs via
`cargo component build --target wasm32-wasip2` (inside the host-stubs
workspace) and adds them to the `wac plug` chain after the secure-log
stack. After composition the orchestrator's residual imports are:

- All standard WASI interfaces (`wasi:filesystem`, `wasi:clocks`,
  `wasi:random`, `wasi:io`, `wasi:cli`).
- `sys:compose/types@1.0.0` -- type-only contract with no functions;
  contributes nothing at instantiation time.

The Java host (webassembly4j) only needs to wire WASI; the composed
orchestrator instantiates without any custom component-model host
functions.

### Replacing a stub with a real implementation

Every stub path in `build.sh` is overridable via an env var
(`RUNTIME_INFO_STUB_WASM`, `BOOTSTRAP_STUB_WASM`,
`RUNTIME_CONTROL_STUB_WASM`). A downstream consumer that ships a real
runtime backend or a real bootstrap adapter can drop the corresponding
wasm into the plug chain without editing the script or recompiling the
orchestrator.

The stubs are static-return today because the orchestrator uses each
import in a narrow way (fingerprint mixed into the exec-key; args
inspected for count only; runtime constructor referenced only as a
function pointer under `core::hint::black_box`). As the orchestrator's
Rust implementation grows to actually drive `tegmentum:runtime` for
in-guest instantiation, the stubs will need real behaviour -- at which
point real adapter components replace them and the stubs stay as the
minimum-viable fallback for embedders that opt out of that feature.

## Future Enhancements

1. **Parallel composition**: Compose multiple plans concurrently
2. **Streaming**: Handle large components without loading entirely in memory
3. **Incremental**: Reuse sub-compositions for faster rebuilds
4. **Upstream PR**: Contribute bytes-based API back to wasm-tools

## Resources

- **Main Project**: https://github.com/tegmentum/webassembly-compoent-orchestration
- **Forked wasm-tools**: https://github.com/tegmentum/wasm-tools/tree/bytes-api
- **Component Model Spec**: https://github.com/WebAssembly/component-model
- **wasm-compose Docs**: https://docs.rs/wasm-compose/
