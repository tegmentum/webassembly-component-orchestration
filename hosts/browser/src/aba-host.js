// Browser host for the aba-dynlink SPIKE (#218) — the jco arm.
//
// Proves that the SAME compose:dynlink machinery validated for the echo
// provider also loads + dispatches a REAL sqlink sqlite:extension scalar
// extension (aba) in a headless browser:
//
//   * provider = aba-provider.js  (the aba-endpoint adapter composed with
//     the real aba sqlite:extension component; exports
//     compose:dynlink/endpoint). Self-instantiates ONCE at import; its
//     `endpoint.handle(method, payload) -> Uint8Array` is the shared
//     SQLite-SPI-behind-a-provider compute surface.
//   * guest    = aba-guest.js  (the flavor-B dlopen harness; imports
//     compose:dynlink/linker). It resolves "aba", sends a CBOR `describe`
//     then CBOR `call` envelopes, and prints the scalar results.
//
// Reuses the exact linker wiring from dynlink-host.js: a JS Instance class
// whose `invoke(method, payload)` passes bytes straight through to the
// provider's endpoint.handle. The CBOR envelope rides those bytes; the
// browser host never interprets them — identical to the wasmtime host.

import { buildLinker } from './dynlink-host.js'
import { buildWasiImports } from './host-imports.js'

const GUEST_URL = '../transpiled/aba-guest/guest.js'
const PROVIDER_URL = '../transpiled/aba-provider/provider.js'

let providerImportCount = 0
let providerModulePromise = null

async function getSharedProvider() {
  if (!providerModulePromise) {
    providerModulePromise = (async () => {
      providerImportCount += 1
      const mod = await import(/* @vite-ignore */ PROVIDER_URL)
      return mod.endpoint.handle
    })()
  }
  return providerModulePromise
}

export async function runAbaGuest(hooks = {}) {
  const handle = await getSharedProvider()
  const { linker, stats } = buildLinker(handle, hooks)

  let stdout = ''
  let stderr = ''
  const decoder = new TextDecoder()
  const { imports: wasiImports } = await buildWasiImports({
    onStdout: (data) => {
      stdout += decoder.decode(data, { stream: true })
    },
    onStderr: (data) => {
      stderr += decoder.decode(data, { stream: true })
    },
  })

  const imports = {
    'compose:dynlink/linker': linker,
    ...wasiImports,
  }

  const guestMod = await import(/* @vite-ignore */ GUEST_URL)

  const guestDir = new URL('/transpiled/aba-guest/', location.origin)
  const getCoreModule = async (path) => {
    const url = new URL(path, guestDir)
    const resp = await fetch(url)
    const ct = resp.headers.get('content-type') || ''
    if (!resp.ok || !ct.includes('wasm')) {
      throw new Error(
        `failed to fetch core module ${path} from ${url}: ` +
          `status=${resp.status} content-type=${ct}`,
      )
    }
    return WebAssembly.compile(await resp.arrayBuffer())
  }

  const instantiateCore = async (module, importObj) =>
    WebAssembly.instantiate(module, importObj)

  const root = await guestMod.instantiate(getCoreModule, imports, instantiateCore)

  const runResult = root.run.run()
  if (runResult && typeof runResult.then === 'function') {
    await runResult
  }

  return { stdout, stderr, stats }
}

export async function runAbaDemo() {
  const resolves = []
  const invokes = []
  const hooks = {
    onResolve: (id) => resolves.push(id),
    onInvoke: (m) => invokes.push(m),
  }

  const run = await runAbaGuest(hooks)

  return {
    stdout: run.stdout.trim(),
    stderr: run.stderr.trim(),
    stats: run.stats,
    providerImportCount,
    resolves,
    invokes,
  }
}
