# Repository Guidelines

## Project Structure & Module Organization
Source blueprints live at the repo root (`SPEC.md`, `TASKS.md`), with implementation targets organized by domain: WIT definitions under `wit/`, Rust hosts under `hosts/`, CLI tooling in `tools/`, and reproducible fixtures in `examples/` and `conformance/`. Create missing folders before adding code; keep interface packages isolated so canonical hashing and release packaging stay reproducible.

## Build, Test, and Development Commands
Use Rust nightly-compatible toolchains. Build canonicalization crates with `cargo build --release` from `wit/canon-*`, and the reference host via `cargo build --workspace` in `hosts/wasmtime`. Validate plans with `composectl plan validate <plan.cbor>`, emit composed artifacts using `composectl emit build`, and run them locally with `composectl exec run` or `serve` as appropriate. For full fixture checks run `cargo run -p conformance-runner -- run --json artifacts/conformance-summary.json`; the runner shells out to `composectl` for each canonical plan and writes a summary JSON alongside human logs. Override the Wasmtime placeholder component path by exporting `COMPOSECTL_PLACEHOLDER_COMPONENT=/absolute/path/to/component.wasm` when testing non-default artifacts. By default the host searches for `target/compose/placeholder.component.wasm` before falling back to the bundled WAT file.

## Coding Style & Naming Conventions
Rust code follows `rustfmt` defaults (4-space indent, imports grouped by crate) and must pass `cargo clippy --all-targets --all-features`. WIT packages use kebab-case package names and snake_case for functions/types; keep identifiers NFC-normalized to match the canonicalization pipeline. CBOR schemas live beside their generators and should be committed in deterministic, pretty-printed form produced by the tooling.

## Testing Guidelines
Write unit tests with `cargo test --all` and add integration suites under `conformance/runner`. Each new plan or interface needs a matching canonicalization vector (`*.cbor` + `sha256.txt`). Use `composectl conformance run --host=wasmtime` before submitting PRs; document any expected skips in `conformance/README.md`.

## Commit & Pull Request Guidelines
No history exists yet, so adopt Conventional Commits (`type(scope): short summary`) to ease changelog generation. Reference issue IDs in the body when applicable and note determinism impacts explicitly. Pull requests must describe affected worlds/interfaces, include CLI/test commands executed, and link to updated artifacts. Attach logs or digests for trust, secrets, or attestation changes so reviewers can reproduce results.

## Security & Configuration Tips
Never commit raw secrets; plans should reference secret tokens only. Store trust root material in `hosts/*/trust/` and document policy changes in `TASKS.md`. When enabling networked secret backends, gate them behind feature flags and confirm audit toggles default to off in development builds.
