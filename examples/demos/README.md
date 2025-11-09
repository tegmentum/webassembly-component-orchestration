# Demonstrations

Interactive demonstrations of key system features.

## Available Demos

### Secrets Management Demo
```bash
./secrets-demo.sh
```

Demonstrates:
- Environment variable injection
- Vault integration (simulated)
- PKCS#11 key access (simulated)
- Multiple secret backends

### Trust & Verification Demo
```bash
./trust-demo.sh
```

Demonstrates:
- Unsigned component rejection
- Signed component acceptance
- Trust backends (SigStore, PGP, X.509)
- Signature verification workflow

### Determinism Demo
```bash
./determinism-demo.sh
```

Demonstrates:
- Strict determinism mode
- Relaxed determinism mode
- Syscall filtering
- Reproducible execution

## Run All Demos

```bash
./run-all-demos.sh
```

## Architecture

Each demo is a shell script that illustrates concepts through:
- Example plans and configurations
- Simulated execution flows
- Policy enforcement examples
- Output showing expected behavior

## Implementation References

- Secrets: `hosts/wasmtime/src/secrets/`
- Trust: `hosts/wasmtime/src/trust/`
- Determinism: `hosts/wasmtime/src/exec.rs`

## Notes

These are educational demos showing architectural concepts. 
Full implementations are in the host code referenced above.
