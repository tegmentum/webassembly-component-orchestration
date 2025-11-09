# Hierarchical Error Codes Registry

**Version:** 1.0.0
**Package:** `sys:compose@1.0.0`

This document defines the canonical error code hierarchy for the Compositional WebAssembly System.

---

## Error Code Structure

Error codes follow a hierarchical naming convention:

```
<Domain>.<Category>[.<Subcategory>]
```

Examples:
- `Plan.Invalid`
- `Emit.MissingBlob`
- `Exec.Trap`

---

## Error Domains

### 1. Plan Domain

Plan validation and structure errors.

| Code | Description | Context |
|------|-------------|---------|
| `Plan.InvalidSchema` | Plan does not conform to CDDL schema | Schema validation details |
| `Plan.InvalidCBOR` | CBOR encoding is not canonical or malformed | Parse error details |
| `Plan.MissingField` | Required field is missing | Field name |
| `Plan.InvalidGraph` | Component graph is invalid | Graph validation details |
| `Plan.CycleDetected` | Circular dependency in component graph | Cycle path |
| `Plan.UnknownComponent` | Referenced component ID not found | Component ID |
| `Plan.InvalidBinding` | Import/export binding is invalid | Binding details |
| `Plan.DuplicateComponent` | Component ID appears multiple times | Component ID |

### 2. Emit Domain

Composition and artifact generation errors.

| Code | Description | Context |
|------|-------------|---------|
| `Emit.MissingBlob` | Required blob not found in storage | Digest |
| `Emit.InvalidDigest` | Blob digest does not match | Expected vs actual |
| `Emit.CompositionFailed` | Component composition failed | Composition details |
| `Emit.LinkError` | Import/export linking failed | Link details |
| `Emit.IncompatibleTypes` | Type mismatch in binding | Type details |
| `Emit.CacheError` | Cache operation failed | Cache details |

### 3. Exec Domain

Execution and runtime errors.

| Code | Description | Context |
|------|-------------|---------|
| `Exec.Trap` | WebAssembly trap occurred | Trap details |
| `Exec.Timeout` | Execution exceeded time limit | Timeout duration |
| `Exec.ResourceExhausted` | Resource limit exceeded | Resource type and limit |
| `Exec.CapabilityDenied` | Required capability denied by policy | Capability name |
| `Exec.MissingExport` | Exported function not found | Export name |
| `Exec.InvalidArgs` | Function arguments are invalid | Argument details |
| `Exec.CacheError` | Execution cache error | Cache details |

### 4. Blob Domain

Content-addressed storage errors.

| Code | Description | Context |
|------|-------------|---------|
| `Blob.NotFound` | Blob not found by digest | Digest |
| `Blob.DigestMismatch` | Computed digest doesn't match expected | Expected vs actual |
| `Blob.IOError` | Storage I/O operation failed | I/O error details |
| `Blob.StorageFull` | Storage capacity exceeded | Storage details |
| `Blob.CorruptedData` | Blob data is corrupted | Corruption details |

### 5. Trust Domain

Verification and signature errors.

| Code | Description | Context |
|------|-------------|---------|
| `Trust.VerificationFailed` | Artifact verification failed | Verification details |
| `Trust.SignatureInvalid` | Signature is invalid or malformed | Signature details |
| `Trust.CertificateExpired` | Certificate has expired | Certificate details |
| `Trust.UntrustedSource` | Source is not in trusted set | Source identity |
| `Trust.BackendError` | Trust backend operation failed | Backend details |
| `Trust.MissingSignature` | Signature required but not provided | Artifact details |

### 6. Secret Domain

Secret management errors.

| Code | Description | Context |
|------|-------------|---------|
| `Secret.NotFound` | Secret not found in backend | Secret ID |
| `Secret.AccessDenied` | Permission denied to access secret | Secret ID and tenant |
| `Secret.BackendError` | Secret backend operation failed | Backend details |
| `Secret.InvalidToken` | Secret token is invalid | Token details |
| `Secret.Expired` | Secret has expired | Expiration details |

### 7. Generic Domain

Generic system errors.

| Code | Description | Context |
|------|-------------|---------|
| `Generic.InvalidInput` | Input validation failed | Input details |
| `Generic.InternalError` | Internal system error | Error details |
| `Generic.NotImplemented` | Feature not yet implemented | Feature name |
| `Generic.Unsupported` | Operation not supported | Operation details |

---

## Error Structure

Errors are represented in WIT as:

```wit
record error {
  code: error-code,
  message: string,
  context: option<string>,
}
```

### Fields

- **code**: Hierarchical error code (enum variant)
- **message**: Human-readable error message
- **context**: Optional JSON-encoded structured context with additional details

### Context Format

The `context` field should contain JSON-serializable data relevant to debugging:

```json
{
  "component_id": "example:component@1.0.0",
  "digest": "sha256:1234...",
  "operation": "validate",
  "details": "..."
}
```

---

## Usage Guidelines

1. **Consistency**: Always use the canonical error code for the specific failure.
2. **Context**: Provide structured context when available to aid debugging.
3. **Messages**: Write clear, actionable error messages.
4. **Events**: Emit error events via the `events` interface for observability.

---

## Error Code Extensions

New error codes can be added following these rules:

1. Must fit within the existing domain hierarchy
2. Must not conflict with existing codes
3. Should be documented with description and context requirements
4. Breaking changes require a major version bump

---

## CBOR Encoding

Error codes are encoded in CBOR as integer enums for efficiency:

```
Plan.InvalidSchema -> 0
Plan.InvalidCBOR -> 1
...
```

The mapping is deterministic and defined in the canonical WIT encoding.

---

**Last Updated:** 2025-01-09
