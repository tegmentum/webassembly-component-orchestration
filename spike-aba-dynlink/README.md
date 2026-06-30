# Spike #218 — aba scalar extension via compose:dynlink

Proves `compose:dynlink` can dynamically link a real sqlink
`sqlite:extension` scalar extension (`aba`, ABA routing-number checks),
modeled as a compose:dynlink resident provider, end-to-end on BOTH the
wasmtime reference host AND the browser jco path — without the bespoke
host-side `extension-loader`.

## Provider model

A scalar `sqlite:extension` component exports
`sqlite:extension/metadata` (`describe`) + `sqlite:extension/scalar-function`
(`call`). compose:dynlink providers instead export the single uniform
`compose:dynlink/endpoint.handle` (bytes in/out, CBOR envelope).

`aba-endpoint/` is the ADAPTER that bridges the two — it IS "the SQLite
connection behind the provider", the minimal SQLite host-SPI a scalar
extension needs (describe + scalar dispatch), modeled exactly like the
existing s3-endpoint / gdal-endpoint resident providers:

* EXPORTS `compose:dynlink/endpoint`
* IMPORTS `sqlite:extension/metadata` + `/scalar-function`, satisfied at
  compose time by the real aba component (wac plug) -> `aba-provider.wasm`

CBOR envelope over `endpoint.handle(method, payload)`:
* `describe` -> CBOR manifest (registered scalars)
* `call` -> CBOR `{ func_id, args:[sql-value] }` -> CBOR `sql-value`

`harness/` is a flavor-B dlopen guest: imports `compose:dynlink/linker`,
resolves "aba", sends describe + call, prints results.

## Build

    ./build.sh        # builds adapter, wac-composes aba-provider.wasm, builds harness

Requires the aba component:
`~/git/sqlink/extensions/aba/target/wasm32-wasip2/release/aba_extension.component.wasm`.

## Run — wasmtime arm

    cargo test -p compose-host-wasmtime --test aba_dynlink_spike -- --nocapture

(drives the real ExecHandler runtime-linkage path -> compose:dynlink linker bridge)

## Run — browser (jco) arm

    cd ../hosts/browser
    node scripts/transpile-aba.mjs
    npx playwright test tests/aba-dynlink.spec.js

Both arms produce identical output:

    loaded extension: aba v0.1.0 (3 scalars)
      scalar id=1 name=aba_validate num_args=1
      ...
    aba_validate('021000021') => 1
    aba_validate('021000022') => 0
