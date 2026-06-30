// Browser (jco) host for task #219 — proves the REUSABLE
// sqlite-extension-endpoint provider bridges DECLARATIVE sqlite:extension
// tiers through compose:dynlink in a headless browser, the SAME machinery
// as the wasmtime arm.
//
//   * provider = one composed <ext>-provider.wasm per tier (the reusable
//     provider shape wac-plug'd with a real sqlink extension; exports
//     compose:dynlink/endpoint). Self-instantiates ONCE at import.
//   * guest    = the generic dlopen harness (imports compose:dynlink/
//     linker). The tier it exercises is chosen by the SCENARIO env var,
//     which we inject through a wasi:cli/environment override.
//
// Reuses buildLinker from dynlink-host.js (the generic linker wiring) and
// buildWasiImports from host-imports.js; the only per-tier inputs are the
// transpiled provider URL and the SCENARIO value.

import { buildLinker } from './dynlink-host.js'
import { buildWasiImports } from './host-imports.js'

export async function runTier(tier) {
  const PROVIDER_URL = `../transpiled/sqlext-${tier}/provider.js`
  const GUEST_URL = '../transpiled/sqlext-guest/guest.js'

  let providerImportCount = 0
  const providerMod = await import(/* @vite-ignore */ PROVIDER_URL)
  providerImportCount += 1
  const handle = providerMod.endpoint.handle

  const resolves = []
  const invokes = []
  const { linker, stats } = buildLinker(handle, {
    onResolve: (id) => resolves.push(id),
    onInvoke: (m) => invokes.push(m),
  })

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

  // Inject SCENARIO into the guest's WASI environment so the one generic
  // harness runs the requested tier (mirrors the host arm's `env` vec).
  // jcoCompat strips the version suffix, so the key is versionless and the
  // guest reads `imports['wasi:cli/environment'].getEnvironment()`.
  wasiImports['wasi:cli/environment'] = {
    ...(wasiImports['wasi:cli/environment'] || {}),
    getEnvironment: () => [['SCENARIO', tier]],
  }

  const imports = {
    'compose:dynlink/linker': linker,
    ...wasiImports,
  }

  const guestMod = await import(/* @vite-ignore */ GUEST_URL)
  const guestDir = new URL('/transpiled/sqlext-guest/', location.origin)
  const getCoreModule = async (path) => {
    const url = new URL(path, guestDir)
    const resp = await fetch(url)
    const ct = resp.headers.get('content-type') || ''
    if (!resp.ok || !ct.includes('wasm')) {
      throw new Error(
        `failed to fetch core module ${path} from ${url}: status=${resp.status} content-type=${ct}`,
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

  return {
    tier,
    stdout: stdout.trim(),
    stderr: stderr.trim(),
    stats,
    providerImportCount,
    resolves,
    invokes,
  }
}

export async function runAllTiers() {
  const tiers = ['scalar', 'aggregate', 'collation', 'vtab', 'hooks']
  const out = {}
  for (const t of tiers) {
    out[t] = await runTier(t)
  }
  return out
}
