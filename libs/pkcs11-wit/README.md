# PKCS#11 WIT Definitions

WebAssembly Interface Type (WIT) definitions for PKCS#11 cryptographic token interface standard.

[![API Coverage](https://img.shields.io/badge/PKCS%2311%20Coverage-~95%25-brightgreen)](https://github.com/tegmentum/pkcs11-wit)
[![Version](https://img.shields.io/badge/PKCS%2311-v2.40-blue)](https://docs.oasis-open.org/pkcs11/pkcs11-base/v2.40/)

## Overview

This repository contains WIT interface definitions that model the PKCS#11 (Cryptoki) API for use with WebAssembly Component Model. These definitions enable WebAssembly components to interact with cryptographic tokens, HSMs, and smart cards through a standardized, type-safe interface.

**Coverage:** ~95% of essential PKCS#11 v2.40 functionality with modern, resource-oriented design improvements over the C API.

## Structure

- `pkcs11-core/` - Core PKCS#11 types, error codes (97 standard errors), and common structures
- `pkcs11-constants/` - All standard PKCS#11 constants (CKM_*, CKA_*, CKO_*, CKK_*, etc.)
- `pkcs11-buffer/` - Buffer management utilities
- `pkcs11-crypto/` - Cryptographic operations with 40+ mechanism parameter types
  - Encryption/decryption (AES-GCM, AES-CCM, AES-CTR, ChaCha20-Poly1305, RSA-OAEP, etc.)
  - Signing/verification (RSA-PSS, ECDSA, etc.)
  - Key derivation (ECDH, PBKDF2, HKDF, TLS-PRF, SSL3, etc.)
  - Hashing and MAC operations
- `pkcs11-object/` - Object management (create, destroy, find, get/set attributes)
- `pkcs11-session/` - Session management and authentication
- `pkcs11-token/` - Slot and token management
- `pkcs11-util/` - Utility functions
- `pkcs11-registry/` - Provider registry
- `worlds/` - WIT world definitions combining all interfaces
- `guest-smoke/` - Example guest interface for testing

## Features

### Comprehensive Coverage
- **97 error codes** covering all PKCS#11 v2.40 standard errors
- **200+ constants** including mechanisms, attributes, object classes, key types, and flags
- **40+ mechanism parameters** for RSA, AES, ECDH, PBKDF2, SSL/TLS, and more
- **Complete cryptographic operations** - encrypt, decrypt, sign, verify, digest, key generation, key derivation

### Modern Design
- **Type-safe** - Strongly typed mechanism parameters and attribute values
- **Resource-oriented** - WIT resources prevent handle leaks and enforce lifecycle management
- **Memory-safe** - No raw pointers; managed buffers
- **Composable** - Clean package separation for selective imports

### Improvements Over C API
- Result types instead of status codes + output parameters
- Streaming operations via resource methods
- One-shot and multipart patterns
- No NULL pointer ambiguity

## Usage

### As a Git Dependency

Reference these WIT definitions in your project's `Cargo.toml`:

```toml
[dependencies]
# Your other dependencies...

[build-dependencies]
wit-bindgen = "0.46"
```

In your `build.rs`:

```rust
use std::path::PathBuf;

fn main() {
    let wit_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("path/to/pkcs11-wit");

    println!("cargo:rerun-if-changed={}", wit_dir.display());
    std::env::set_var("PKCS11_WIT_ROOT", wit_dir);
}
```

### With Git Submodules

Add as a submodule to your project:

```bash
git submodule add https://github.com/your-org/pkcs11-wit.git wit/pkcs11
git submodule update --init --recursive
```

## License

Apache-2.0

## Related Projects

- [wasm-pkcs11](https://github.com/your-org/wasm-pkcs11) - Host adapter implementation
