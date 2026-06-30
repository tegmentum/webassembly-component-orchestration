# sqlite-extension-endpoint (#219)

A **reusable, parameterized** `compose:dynlink` provider that bridges
**all declarative** `sqlite:extension@1.0.0` tiers to the uniform
`compose:dynlink/endpoint` bytes envelope. Generalizes the spike #218
aba-specific adapter: any declarative extension can now be loaded through
the generic dynlink linker, retiring the bespoke `extension-loader` for
those tiers.

## Design — the generic envelope dispatch table

A declarative `sqlite:extension` component EXPORTS tier interfaces
(`metadata`, `scalar-function`, `aggregate-function`, `collation`,
`vtab`, `vtab-update`, `authorizer`, `update-hook`, `commit-hook`,
`wal-hook`, `dot-command`). A `compose:dynlink` provider instead exports
the single uniform `endpoint.handle(method, payload) -> bytes`. This
provider bridges the two:

* it EXPORTS `compose:dynlink/endpoint`,
* it IMPORTS the declarative tier interfaces it bridges — satisfied at
  **compose time** by the real extension via `wac plug`, **never
  recompiled into the provider per extension**.

The bridging logic (the CBOR envelope + the dispatch table) is written
**once** in [`provider/src/envelope.rs`](provider/src/envelope.rs) +
[`provider/src/lib.rs`](provider/src/lib.rs). The method table:

| method | maps to |
| --- | --- |
| `manifest` / `describe` | `metadata.describe` -> full manifest (every tier's registered entries) |
| `policy-check` | manifest -> grant reconcile, **fail-closed** |
| `call` | `scalar-function.call` |
| `agg.step` / `agg.finalize` / `agg.value` / `agg.inverse` | `aggregate-function` lifecycle |
| `collation.compare` | `collation.compare` |
| `vtab.create` / `connect` / `best-index` / `open` / `filter` / `next` / `eof` / `column` / `rowid` / `fetch-batch` / `close` / `disconnect` / `destroy` | `vtab` (read) |
| `vtab-update.update` / `begin` / `sync` / `commit` / `rollback` | `vtab-update` (mutating) |
| `authorizer.authorize` | `authorizer.authorize` |
| `hook.update` / `hook.commit` / `hook.rollback` / `hook.wal` | hook callbacks |
| `dotcmd.invoke` | `dot-command.invoke` (+ captured cli-stdout/stderr) |

### Parameterization: one source, a small fixed set of world shapes

`wac plug` leaves any unsatisfied import in place, and the dynlink host
cannot satisfy a leftover `sqlite:extension/*` import — so a single
kitchen-sink world that imports every tier would, when plugged with an
extension that only exports a subset, leave dangling imports that block
instantiation. The provider is therefore offered as a small fixed set of
**world shapes** (one Cargo feature each), all compiling the **same
source** — only the `generate!` world differs:

`scalar`, `aggregate`, `collation`, `vtab`, `vtab-mut`, `hooks`, `dotcmd`
(see [`provider/wit/world.wit`](provider/wit/world.wit)).

Pick the shape whose import set matches the extension's export set at
compose time. `metadata` is imported by every shape (the
manifest->register reconcile + policy mapping run off `describe()`).

### Manifest -> register reconciliation + policy/capability mapping

`describe` returns the full declarative manifest — every registered entry
across every tier plus `declared-capabilities` / `optional-capabilities`.
This is the describe->register flow the bespoke loader performs, here
expressed over the endpoint: the manifest drives what the host/SQLite
side registers.

`policy-check` mirrors the bespoke loader's `policy_from_load_options`:
it reads the manifest's `declared-capabilities`, compares against the
compose:dynlink plan's grant set, and **fails closed** if any required
capability is not granted (`Manifest::reconcile_policy` in
`envelope.rs`). `optional-capabilities` are reported but never gate the
load. The harness demonstrates the gate: granting the declared set
passes; an empty grant against a non-empty declared set is refused.

## Tier coverage — what is proven, both hosts

Every tier below is proven **end-to-end with actual output** on the
wasmtime reference host (`hosts/wasmtime/tests/
sqlite_ext_endpoint_declarative.rs`) AND the browser jco host
(`hosts/browser/tests/sqlext-dynlink.spec.js`).

| tier | shape | extension | proof |
| --- | --- | --- | --- |
| scalar | `scalar` | `aba` (catalog) | `aba_validate('021000021') => 1` |
| aggregate | `aggregate` | `count_min` (catalog) | step x5 + finalize -> 32768B sketch; `count_min_estimate(sketch,'apple') => 3` |
| collation | `collation` | `uint` (catalog) | natural-numeric `x2 < x10` |
| vtab (read) | `vtab` | `series` (catalog) | `generate_series(1,5) => [1,2,3,4,5]` |
| vtab (mutating) | `vtab-mut` | `inmem` (catalog) | create + xUpdate INSERT x2 -> `SELECT key,value FROM inmem => [(alpha=100),(beta=200)]` |
| hooks | `hooks` | `hookcb` (test ext) | `authorize(read,'t')=>ok`, `authorize(read,'secret')=>deny`, update dispatched, `commit veto=false`, `wal rc=0` |
| dot-command | `dotcmd` | `dotret` (test ext) | `.echo hello world => "echo: hello world"` |

`describe` (full manifest) + `policy-check` (fail-closed reconcile) are
exercised on every tier.

### Test extensions vs catalog extensions

Five tiers use real sqlink catalog extensions. Two use tiny purpose-built
test extensions under [`test-extensions/`](test-extensions/), each for a
documented reason:

* **`hookcb`** (hooks) — the catalog hook extension `hookprobe` drags in
  `spi` / `wal-frames` / `s3-base` host imports via its *reentrant scalar
  probes*; those would be leftover unsatisfied imports. `hookcb`
  exercises the same authorizer + update/commit/wal **callback** surface
  with no reentrant calls, so the composed provider has zero leftover
  imports. The non-reentrant hook-callback surface is fully covered; the
  reentrant scalar-probe surface is the reentrant tier #220's job.
* **`dotret`** (dot-command) — see the reentrancy note below.

## Reentrancy boundaries (hand-off to #220)

Two surfaces turned out to need host SPI the declarative bridge does not
provide; they are flagged for the reentrant tier (#220):

1. **Hook extensions whose SCALAR PROBES call back into SQLite**
   (`hookprobe` imports `spi` / `wal-frames` / `s3-base`). The hook
   CALLBACKS themselves are non-reentrant and are covered here via
   `hookcb`; the reentrant probes are not.
2. **STREAMING dot-commands** that emit output via the `cli-stdout` host
   import (e.g. the catalog `greet`). The provider EXPORTS
   `cli-stdout`/`cli-stderr`/`cli-state` to capture that output, but the
   resulting dependency is **cyclic** (provider provides cli-stdout AND
   consumes dot-command) and `wac` cannot express the cycle. Capturing
   streamed output requires the **host** to wire the extension's
   cli-stdout import to the provider's cli-stdout export — exactly what
   the bespoke loader does (it owns the CLI streams). The non-streaming
   path (output returned in `invoke-result.text`) is fully covered here
   via `dotret`; the `dotcmd` shape's cli-stdout capture machinery is
   built and ready for host-mediated wiring.

## Production-ready vs. still-bespoke

* **Production-ready via this provider (no bespoke loader needed):**
  scalar, aggregate, collation, vtab (read), vtab (mutating, self-
  contained), the hook-callback surface, and non-streaming dot-commands —
  any declarative extension whose tier interfaces are self-contained
  (only `types`/`policy` type-imports survive after `wac plug`).
* **Still routes through the bespoke loader / awaits #220:** extensions
  that call back into SQLite host SPI (`spi`/`prepared`/`transaction`/
  `schema`/`http`/`dns`/...) from inside any callback, and streaming
  dot-commands (until host-mediated cli-stream wiring lands).

## Build & run

```sh
./build.sh                 # builds every shape + composes one provider per tier + the harness
# wasmtime arm:
cargo test -p compose-host-wasmtime --test sqlite_ext_endpoint_declarative -- --nocapture
# browser arm:
cd ../hosts/browser
node scripts/transpile-sqlite-ext.mjs
npx playwright test tests/sqlext-dynlink.spec.js
```

The generic harness ([`harness/`](harness/)) is one flavor-B dlopen
guest; the tier it exercises is selected by the `SCENARIO` env var
(threaded into the guest's WASI environment on both hosts).
