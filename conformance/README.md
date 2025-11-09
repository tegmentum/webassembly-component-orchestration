# Conformance Suite

This directory tracks canonicalization vectors and the executable conformance runner used to validate hosts.

- `vectors/` stores deterministic CBOR plans with their reference hashes. Every vector must include a `.cbor` payload and a `.sha256.txt` file containing the canonical digest generated with `shasum -a 256`.
- `runner/` contains the harness that exercises hosts via `composectl conformance run`.

## Workflow

1. Add or update a plan under `vectors/` using the canonical CBOR profile.
2. Regenerate the digest (`shasum -a 256 <file.cbor> > <file>.sha256.txt`).
3. Record the change and rationale in pull requests so reviewers can reproduce the vector.
4. Run `cargo test --all` and `composectl plan validate <vector.cbor>` before opening a PR.

## Planned Conformance Runner Work

- [ ] Implement harness to iterate fixtures (`hello`, `nested`, `large-int`, `multi-component`) and assert expected pass/fail outcomes.
- [ ] Verify duplicate/unsorted fixtures emit canonical ordering errors.
- [ ] Capture component id uniqueness checks through host-side emit pipeline once available.

Document expected skips or host-specific adaptations here when updating the suite.
