# Conformance Runner

This crate provides a CLI skeleton for executing the canonical CBOR fixtures against `composectl`.

## Usage

List known fixtures:

```bash
cargo run -p conformance-runner -- list
```

Run the current skeleton (invokes `composectl plan validate` for each fixture):

```bash
cargo run -p conformance-runner -- run
cargo run -p conformance-runner -- run --fixture nested
```

The runner expects the workspace to be built so that `target/compose/placeholder.component.wasm` and the `composectl` binary are available. The validation logic currently shells out to the `composectl` CLI; future work will integrate structured result reporting.
