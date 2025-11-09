# Wasmtime Host

This crate implements the reference compositor host on top of Wasmtime. It should:

- expose `plan`, `emit`, and `exec` entry points that enforce determinism modes;
- plug into `std:secrets`, `std:metrics`, and optional audit/attest worlds;
- manage trust policies via the files under `trust/` (certs, key fingerprints, policy manifests).

Build with `cargo build --workspace` from the repository root once the Cargo manifest is added.
