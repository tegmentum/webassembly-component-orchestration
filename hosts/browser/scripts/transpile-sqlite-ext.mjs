// Transpile the sqlite-extension-endpoint artifacts to JS via jco for the
// browser (jco) arm of task #219.
//
//   guest    = sqlite-extension-endpoint/harness  (wasi:cli/run, imports
//              compose:dynlink/linker) -> async instantiation + jspi
//   provider = one composed <ext>-provider.wasm per declarative tier
//              (the reusable provider shape `wac plug`'d with a real
//              sqlink extension; exports compose:dynlink/endpoint)
//
// Writes transpiled/sqlext-guest + transpiled/sqlext-<tier>.

import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, '..')
const repoRoot = resolve(root, '..', '..')

const JCO =
  process.env.JCO ||
  resolve(process.env.HOME || '', 'git/ducklink/web/node_modules/.bin/jco')

const MOD = resolve(repoRoot, 'sqlite-extension-endpoint')
const GUEST_WASM = resolve(
  MOD,
  'harness/target/wasm32-wasip2/release/sqlite-ext-endpoint-harness.wasm',
)

// tier -> composed provider file (matches build.sh outputs).
const TIERS = {
  scalar: 'aba-provider.wasm',
  aggregate: 'count_min-provider.wasm',
  collation: 'uint-provider.wasm',
  vtab: 'series-provider.wasm',
  hooks: 'hookcb-provider.wasm',
}

function jco(args) {
  console.log('jco', args.join(' '))
  execFileSync(JCO, args, { stdio: 'inherit' })
}

if (!existsSync(GUEST_WASM)) {
  throw new Error(`missing harness: ${GUEST_WASM} (run sqlite-extension-endpoint/build.sh first)`)
}

jco([
  'transpile',
  GUEST_WASM,
  '--name',
  'guest',
  '--instantiation',
  'async',
  '--async-mode',
  'jspi',
  '--no-namespaced-exports',
  '-o',
  resolve(root, 'transpiled/sqlext-guest'),
])

for (const [tier, file] of Object.entries(TIERS)) {
  const provider = resolve(MOD, 'dist/providers', file)
  if (!existsSync(provider)) {
    console.warn(`skip tier ${tier}: missing ${provider}`)
    continue
  }
  jco([
    'transpile',
    provider,
    '--name',
    'provider',
    '--no-namespaced-exports',
    '-o',
    resolve(root, `transpiled/sqlext-${tier}`),
  ])
}
