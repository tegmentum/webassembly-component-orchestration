// Transpile the aba-dynlink SPIKE components to JS via jco, for the
// browser arm of the compose:dynlink spike (#218).
//
//   guest    = spike-aba-dynlink/harness  (wasi:cli/run, imports
//              compose:dynlink/linker) -> async instantiation + jspi
//   provider = spike-aba-dynlink/aba-provider.wasm (the aba-endpoint
//              adapter composed with the real aba sqlite:extension
//              component; exports compose:dynlink/endpoint) -> default mode
//
// Mirrors transpile.mjs but points at the spike artifacts and writes to
// transpiled/aba-guest + transpiled/aba-provider.

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

const SPIKE = resolve(repoRoot, 'spike-aba-dynlink')
const GUEST_WASM = resolve(
  SPIKE,
  'harness/target/wasm32-wasip2/release/aba-dlopen-harness.wasm',
)
const PROVIDER_WASM = resolve(SPIKE, 'aba-provider.wasm')

function jco(args) {
  console.log('jco', args.join(' '))
  execFileSync(JCO, args, { stdio: 'inherit' })
}

for (const w of [GUEST_WASM, PROVIDER_WASM]) {
  if (!existsSync(w)) {
    throw new Error(`missing spike component: ${w} (run spike-aba-dynlink/build.sh first)`)
  }
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
  resolve(root, 'transpiled/aba-guest'),
])

jco([
  'transpile',
  PROVIDER_WASM,
  '--name',
  'provider',
  '--async-mode',
  'jspi',
  '-o',
  resolve(root, 'transpiled/aba-provider'),
])

console.log('aba transpile done -> transpiled/aba-guest, transpiled/aba-provider')
