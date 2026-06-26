// Re-transpile the dynlink guest + provider components to JS via jco.
//
// The guest is a wasi:cli/run CLI that imports compose:dynlink/linker;
// it MUST be transpiled in async-instantiation mode so the host can
// supply the linker (a resource-returning import) as a JS object/class
// at instantiate() time:
//
//   jco transpile --instantiation async --async-mode jspi ...
//
// The provider exports compose:dynlink/endpoint and is transpiled in
// default mode (self-instantiates once at module import).
//
// jco binary: resolved from the local ducklink web toolchain (jco 1.15.x)
// or any jco on PATH. Override with JCO=/path/to/jco.

import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, '..')
const repoRoot = resolve(root, '..', '..')

const JCO =
  process.env.JCO ||
  resolve(
    process.env.HOME || '',
    'git/ducklink/web/node_modules/.bin/jco',
  )

const GUEST_WASM = resolve(
  repoRoot,
  'examples/dynlink-dlopen-guest/target/wasm32-wasip2/release/dynlink-dlopen-guest.wasm',
)
const PROVIDER_WASM = resolve(
  repoRoot,
  'examples/dynlink-echo-provider/target/wasm32-wasip2/release/dynlink_echo_provider.wasm',
)

function jco(args) {
  console.log('jco', args.join(' '))
  execFileSync(JCO, args, { stdio: 'inherit' })
}

for (const w of [GUEST_WASM, PROVIDER_WASM]) {
  if (!existsSync(w)) {
    throw new Error(`missing prebuilt component: ${w} (build the example first)`)
  }
}

// Guest: async instantiation + jspi so the host supplies the linker
// import (resource-returning) and CLI stream blocking ops can suspend.
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
  resolve(root, 'transpiled/guest'),
])

// Provider: default mode (self-instantiating module export).
jco([
  'transpile',
  PROVIDER_WASM,
  '--name',
  'provider',
  '--async-mode',
  'jspi',
  '-o',
  resolve(root, 'transpiled/provider'),
])

console.log('transpile done -> transpiled/guest, transpiled/provider')
