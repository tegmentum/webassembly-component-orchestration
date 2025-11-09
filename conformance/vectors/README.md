# Canonical Vectors

Store CBOR artifacts and their `sha256.txt` digests here. Keep filenames descriptive (`<scenario>.cbor`) and regenerate hashes whenever canonicalization algorithms change.

## Current Vectors

- `hello-plan.cbor` / `hello-plan.sha256.txt`: minimal plan exercising the CLI bootstrap path.
- `nested-plan.cbor` / `nested-plan.sha256.txt`: canonical fixture with nested component maps for ordering tests.
- `large-int-plan.cbor` / `large-int-plan.sha256.txt`: plan containing large integer limits to confirm canonical encoding of big values.
- `multi-component-plan.cbor` / `multi-component-plan.sha256.txt`: multi-component graph with provides/requires ordered canonically.
- `multi-component-plan-unsorted.cbor` / `multi-component-plan-unsorted.sha256.txt`: intentionally unsorted requires keys to ensure canonical ordering is enforced.
- `duplicate-plan.cbor` / `duplicate-plan.sha256.txt`: plan with repeated component IDs for negative uniqueness tests.

To create additional vectors:

```bash
python3 scripts/make-plan.py   # or use composectl once implemented
shasum -a 256 conformance/vectors/<name>.cbor > conformance/vectors/<name>.sha256.txt
```

Commit both files together so CI can validate them.
