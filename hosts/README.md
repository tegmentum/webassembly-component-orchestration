# Host Runtimes

Reference host implementations live here. Each host must expose the `sys:compose` world, enforce policy hooks, and integrate with the canonicalization toolchain.

- `wasmtime/` is the initial Rust host; add platform-specific notes in its README.
- Additional hosts should mirror this structure and document required feature flags and trust roots.

Keep trust material, cached artifacts, or secrets out of the repository; only ship configuration templates.
