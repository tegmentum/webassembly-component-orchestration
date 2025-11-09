# Security & Hardening

Security considerations and hardening measures for the WebAssembly Compositional System.

## Resource Limits & DOS Protection

### System-Wide Limits

The system implements multiple layers of protection against denial-of-service attacks:

**Plan Limits:**
- Maximum plan file size: 1MB
- Maximum components per plan: 1,000
- Maximum bindings: 10,000
- Maximum graph depth: 100 levels

**Blob Limits:**
- Maximum blob size: 100MB
- Total system memory limit: 1GB

### Implementation

See `hosts/wasmtime/src/limits.rs` for full implementation.

```rust
pub struct SystemLimits {
    pub max_plan_size: usize,           // 1MB
    pub max_components: usize,          // 1,000
    pub max_bindings: usize,            // 10,000
    pub max_graph_depth: usize,         // 100 levels
    pub max_blob_size: u64,             // 100MB
    pub max_total_memory: u64,          // 1GB
}
```

## Input Validation

Multi-phase validation pipeline ensures no panic or OOM on invalid input:

### Validation Phases

1. **Schema Validation**
   - Version checking
   - Required fields present
   - Type correctness

2. **Blob Availability**
   - All components present in blob store
   - SHA-256 digest verification

3. **Graph Structure**
   - Cycle detection
   - Connectivity validation
   - Depth limits

4. **Binding Validation**
   - References point to valid components
   - Import/export compatibility
   - No dangling bindings

## Capability System

### Least Privilege

- No capabilities granted by default
- Explicit opt-in required via policy
- Policy-based filtering at runtime
- Capability violations logged to audit trail

### Isolation

- Tenant separation via tenant_id
- Cache isolation per tenant
- Audit log separation
- Metric namespacing

## Secret Management

### Security Principles

- Secrets never logged or exposed in errors
- Token-based access only
- Backend-specific validation
- Lifetime limits enforced

### Supported Backends

- Environment variables (dev only)
- HashiCorp Vault
- PKCS#11 hardware tokens
- Custom backends via plugin interface

## Trust & Verification

### Verification Methods

- SigStore (Cosign)
- PGP/GPG signatures
- X.509 certificates
- Custom verifiers

### Component Verification

```rust
pub struct ComponentVerification {
    pub digest: Vec<u8>,           // SHA-256 of component
    pub signature: Option<Vec<u8>>,
    pub verifier: Option<String>,  // Which backend verified
}
```

## Audit Logging

### What is Logged

- Plan validation attempts
- Blob storage operations
- Secret access requests
- Policy enforcement decisions
- Component execution events
- Capability grant/deny decisions

### Log Format

```json
{
  "timestamp": "2024-01-01T12:00:00Z",
  "tenant_id": "tenant-123",
  "event_type": "secret_access",
  "component_id": "comp-abc",
  "details": {
    "secret_id": "api-key",
    "backend": "vault://secrets",
    "result": "granted"
  }
}
```

## Multi-Tenancy

### Isolation Guarantees

- Each tenant has isolated:
  - Blob storage namespace
  - Cache directory
  - Audit logs
  - Metrics namespace
  - Secret access scope

### Resource Limits

Per-tenant limits can be configured via policy:

```rust
pub struct TenantLimits {
    pub max_components: usize,
    pub max_memory_bytes: u64,
    pub max_execution_time_ms: u64,
}
```

## Known Limitations

### Current Implementation

1. **Fuzzing Coverage**
   - Plan parser not comprehensively fuzzed
   - WIT canonicalization needs fuzzing
   - Status: Identified, not yet addressed

2. **Performance Profiling**
   - No formal benchmarking suite
   - Cache optimization opportunities remain
   - Status: Informal testing only

3. **Edge Cases**
   - Some resource exhaustion scenarios may exist
   - Complex graph patterns not fully tested
   - Status: Basic coverage complete

### Future Work

- [ ] Comprehensive fuzzing campaign (AFL, libFuzzer)
- [ ] Formal performance benchmarking suite
- [ ] Advanced DOS protection (rate limiting)
- [ ] Side-channel resistance analysis
- [ ] Formal verification of critical paths

## Threat Model

### Mitigated Threats

- ✅ Malicious plans (validated, rejected)
- ✅ Resource exhaustion (limited, bounded)
- ✅ Capability violations (enforced, logged)
- ✅ Secret leakage (protected, never exposed)
- ✅ Unauthorized component execution (policy enforced)

### Unmitigated Threats

- ⚠️ Side-channel attacks (timing, cache)
- ⚠️ Speculative execution vulnerabilities
- ⚠️ Physical access to blob storage
- ⚠️ Compromised host environment

## Security Best Practices

### For Production Use

1. **Enable Audit Logging**
   ```rust
   let audit_logger = AuditLogger::new(PathBuf::from("/var/log/compositor"))?;
   ```

2. **Set Conservative Resource Limits**
   ```rust
   let limits = SystemLimits {
       max_plan_size: 512 * 1024,      // 512KB
       max_components: 100,             // Lower for production
       max_bindings: 1000,
       ..Default::default()
   };
   ```

3. **Require Component Signatures**
   ```rust
   let policy = HostPolicy {
       require_signatures: true,
       allowed_verifiers: vec!["sigstore".to_string()],
       ..Default::default()
   };
   ```

4. **Isolate Tenants**
   - Use separate blob directories per tenant
   - Configure tenant-specific limits
   - Monitor cross-tenant isolation

5. **Monitor Metrics for Anomalies**
   - Track execution times
   - Monitor memory usage
   - Alert on policy violations

6. **Regular Security Updates**
   - Keep Wasmtime updated
   - Update dependencies regularly
   - Review security advisories

## Security Reporting

To report security vulnerabilities, please email: security@example.com

Do not open public issues for security vulnerabilities.

## Compliance

### SLSA Level 2

- ✅ Build provenance tracking
- ✅ Signed attestations (see `attest.rs`)
- ✅ Reproducible builds (strict determinism mode)

### References

- SLSA: https://slsa.dev/
- WebAssembly Security: https://webassembly.org/docs/security/
- OWASP Top 10: https://owasp.org/www-project-top-ten/
- CWE Top 25: https://cwe.mitre.org/top25/
