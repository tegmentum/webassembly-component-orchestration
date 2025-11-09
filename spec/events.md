# Event Record Format Specification

**Version:** 1.0.0
**Package:** `sys:compose@1.0.0`

This document defines the structured event format for the Compositional WebAssembly System.

---

## Overview

Events provide structured telemetry and observability throughout the composition and execution lifecycle. They are emitted via the `events` interface and collected by the host for logging, metrics, and audit purposes.

---

## Event Structure

Events are represented in WIT as:

```wit
record event {
  level: event-level,
  timestamp: u64,
  message: string,
  context: option<string>,
}
```

### Fields

#### 1. `level` (event-level)

Severity level of the event:

```wit
enum event-level {
  trace,
  info,
  warn,
  error,
}
```

| Level | Purpose | Examples |
|-------|---------|----------|
| `trace` | Detailed debugging information | Function entry/exit, variable states |
| `info` | General informational messages | Operation started, operation completed |
| `warn` | Warning conditions that don't prevent operation | Deprecated feature used, suboptimal configuration |
| `error` | Error conditions | Operation failed, invalid input |

#### 2. `timestamp` (u64)

Unix timestamp in milliseconds (milliseconds since epoch).

- **Format**: 64-bit unsigned integer
- **Precision**: Milliseconds
- **Example**: `1704844800000` (2024-01-10 00:00:00 UTC)

**Generation**:
```rust
use std::time::{SystemTime, UNIX_EPOCH};
let timestamp = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .as_millis() as u64;
```

#### 3. `message` (string)

Human-readable event message.

**Guidelines**:
- Clear and concise
- Present tense for current actions ("validating plan")
- Past tense for completed actions ("validated plan")
- Include relevant identifiers (component IDs, digests)
- Avoid sensitive information (secrets, tokens)

**Examples**:
```
"validating plan schema"
"composed 3 components successfully"
"trust verification failed for component example:foo@1.0.0"
```

#### 4. `context` (option<string>)

Optional JSON-encoded structured context providing additional details.

**Format**: JSON object as string
**Purpose**: Machine-readable event metadata for structured logging and analysis

---

## Context Field Schema

The `context` field should contain a JSON object with relevant metadata:

### Common Context Fields

```json
{
  "operation": "string",      // Current operation
  "component_id": "string",   // Component identifier
  "plan_digest": "string",    // Plan digest (hex)
  "emit_key": "string",       // Emit cache key (hex)
  "exec_key": "string",       // Exec cache key (hex)
  "tenant_id": "string",      // Tenant identifier
  "duration_ms": number,      // Operation duration
  "phase": "string"           // Validation/execution phase
}
```

### Operation-Specific Context

#### Plan Validation

```json
{
  "operation": "validate_plan",
  "plan_digest": "abc123...",
  "phase": "graph_validation",
  "component_count": 5,
  "binding_count": 8
}
```

#### Composition (Emit)

```json
{
  "operation": "compose",
  "plan_digest": "abc123...",
  "emit_key": "def456...",
  "component_count": 5,
  "output_size_bytes": 1048576,
  "duration_ms": 250
}
```

#### Execution

```json
{
  "operation": "exec_run_cli",
  "plan_digest": "abc123...",
  "exec_key": "ghi789...",
  "exit_code": 0,
  "duration_ms": 1500,
  "tenant_id": "tenant-123"
}
```

#### Trust Verification

```json
{
  "operation": "verify_trust",
  "digest": "jkl012...",
  "signer": "example.com",
  "backend": "sigstore",
  "verified": true
}
```

#### Error Context

```json
{
  "operation": "validate_plan",
  "error_code": "Plan.CycleDetected",
  "cycle_path": ["comp1", "comp2", "comp3", "comp1"],
  "details": "Circular dependency detected in component graph"
}
```

---

## Event Emission Patterns

### Trace Events

Used for fine-grained debugging:

```rust
events::trace(
    "entering validation phase",
    Some(json!({
        "phase": "schema_validation",
        "plan_digest": digest_hex
    }).to_string())
);
```

### Info Events

Used for significant operations:

```rust
events::info(
    "plan validated successfully",
    Some(json!({
        "plan_digest": digest_hex,
        "component_count": plan.components.len(),
        "duration_ms": duration
    }).to_string())
);
```

### Warn Events

Used for non-critical issues:

```rust
events::warn(
    "optional capability denied",
    Some(json!({
        "capability": "wasi:http",
        "policy": "restricted"
    }).to_string())
);
```

### Error Events

Used for failures:

```rust
events::error(
    "composition failed",
    Some(json!({
        "error_code": "Emit.LinkError",
        "component_id": comp_id,
        "details": error_details
    }).to_string())
);
```

---

## CBOR Encoding

Events are encoded in canonical CBOR for storage and transmission:

```cddl
event = {
  0: event-level,     ; level (integer enum)
  1: uint,            ; timestamp (u64)
  2: tstr,            ; message
  ? 3: tstr,          ; context (optional JSON string)
}

event-level = &(
  trace: 0,
  info: 1,
  warn: 2,
  error: 3,
)
```

---

## Event Collection and Processing

### Host Responsibilities

1. **Collection**: Capture all emitted events
2. **Storage**: Store events for audit and analysis
3. **Forwarding**: Optional forwarding to external logging systems
4. **Filtering**: Filter by level based on configuration
5. **Aggregation**: Aggregate events for metrics

### Event Lifecycle

```
Component emits event
    ↓
Host captures event
    ↓
Local storage (required)
    ↓
External forwarding (optional)
    ↓
Metrics aggregation (optional)
    ↓
Audit trail (required for audit mode)
```

---

## Best Practices

### DO

- Emit events at appropriate levels
- Include structured context for machine processing
- Use consistent operation names
- Include timing information for performance analysis
- Emit both start and completion events for long operations

### DON'T

- Log sensitive data (secrets, tokens, passwords)
- Emit excessive trace events in production
- Include large binary data in context
- Use events as a replacement for proper error handling
- Emit events from hot loops without rate limiting

---

## Example Event Flows

### Plan Validation Flow

```
[trace] "starting plan validation" {operation: "validate", phase: "schema"}
[info] "plan schema validated" {component_count: 3}
[trace] "validating component graph" {phase: "graph"}
[info] "component graph validated" {binding_count: 5}
[trace] "checking for cycles" {phase: "cycles"}
[info] "plan validation complete" {duration_ms: 45}
```

### Composition Flow

```
[info] "starting composition" {component_count: 3}
[trace] "fetching component blobs" {fetched: 1, total: 3}
[trace] "fetching component blobs" {fetched: 2, total: 3}
[trace] "fetching component blobs" {fetched: 3, total: 3}
[info] "linking components" {bindings: 5}
[info] "composition complete" {emit_key: "abc...", size_bytes: 1048576}
```

### Execution Flow

```
[info] "starting execution" {mode: "run-cli"}
[trace] "instantiating component" {component_id: "root"}
[trace] "calling main function" {args: ["--help"]}
[info] "execution complete" {exit_code: 0, duration_ms: 1500}
```

---

## Integration with Audit and Metrics

### Audit Trail

Events with level `error` and `warn` should be included in the audit trail along with:
- Plan digest
- Exec key
- Tenant ID
- Timestamp
- Outcome

### Metrics

Events can be aggregated into metrics:
- **Counters**: Event counts by level and operation
- **Histograms**: Duration distributions from timed events
- **Gauges**: Current state from trace events

---

**Last Updated:** 2025-01-09
