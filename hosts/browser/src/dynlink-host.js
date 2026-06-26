// Browser host for the compose:dynlink orchestration framework.
//
// Proves the "one provider, many guests" composition strategy in a real
// browser: a jco-transpiled guest resolves a provider through a
// JS-implemented `compose:dynlink/linker`, and every resolve hands back
// an Instance backed by the SAME single provider instance.
//
// Wiring (the whole point):
//
//   1. The transpiled provider (default mode) instantiates ITSELF ONCE
//      at module-import time (its emitted JS has a top-level
//      `await $init`). Importing `../transpiled/provider/provider.js`
//      therefore yields a single live core instance whose
//      `endpoint.handle(method, payload) -> Uint8Array` is the shared
//      compute surface. We count that import to assert "instantiated
//      once".
//
//   2. The `compose:dynlink/linker` import the guest needs is shaped (by
//      jco) as `{ Instance, resolveById, resolveByDigest }`. The guest's
//      generated trampoline does `Object.create(Instance.prototype)` /
//      `e instanceof Instance` and then calls `instance.invoke(method,
//      payload)`. So we supply an `Instance` CLASS whose `invoke` calls
//      straight through to the shared `endpoint.handle`. resolveById
//      returns a NEW Instance handle each call but they all close over
//      the one provider — that is the sharing proof.
//
//   3. WASI Preview 2 comes from @tegmentum/wasi-polyfill
//      (see host-imports.js); stdout is captured so we can read the
//      guest's printed `HELLO FROM DLOPEN`.

import { buildWasiImports } from './host-imports.js'

const GUEST_URL = '../transpiled/guest/guest.js'
const PROVIDER_URL = '../transpiled/provider/provider.js'

// Counts how many times the provider component was instantiated. The
// transpiled provider self-instantiates exactly once per ES-module
// evaluation, so a single dynamic import == a single provider instance.
let providerImportCount = 0
let providerModulePromise = null

/**
 * Import (and thereby instantiate) the provider component exactly once,
 * memoised. Returns the shared `endpoint.handle`.
 */
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

/**
 * Build the `compose:dynlink/linker` import object the guest expects.
 *
 * @param {(method: string, payload: Uint8Array) => Uint8Array} handle
 *        the shared provider's endpoint.handle
 * @param {{ onResolve?: (id: string) => void, onInvoke?: (m: string) => void }} [hooks]
 */
export function buildLinker(handle, hooks = {}) {
  // Tracks how many distinct Instance handles were vended and how many
  // invokes flowed through to the one provider — evidence of sharing.
  const stats = { resolves: 0, invokes: 0 }

  // The resource class jco wraps. Each resolve produces a fresh handle,
  // but `invoke` always dispatches into the single shared `handle`.
  class Instance {
    #id
    constructor(id) {
      this.#id = id
    }
    invoke(method, payload) {
      stats.invokes += 1
      if (hooks.onInvoke) hooks.onInvoke(method, this.#id)
      // payload arrives as a Uint8Array; endpoint.handle returns a
      // Uint8Array. Straight pass-through to the shared provider.
      return handle(method, payload)
    }
  }

  function resolveById(id) {
    stats.resolves += 1
    if (hooks.onResolve) hooks.onResolve(id)
    return new Instance(id)
  }

  function resolveByDigest(digest) {
    // Same backing provider, keyed by digest instead of id.
    stats.resolves += 1
    if (hooks.onResolve) hooks.onResolve(digest)
    return new Instance(`digest:${digest}`)
  }

  return {
    linker: { Instance, resolveById, resolveByDigest },
    stats,
  }
}

/**
 * Run the guest once: resolve + invoke the shared provider through the
 * linker, capture and return stdout.
 *
 * @param {{ onResolve?, onInvoke? }} [hooks]
 * @returns {Promise<{ stdout: string, stderr: string, stats: object }>}
 */
export async function runGuest(hooks = {}) {
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

  // The guest is async-instantiation mode:
  //   instantiate(getCoreModule, imports, instantiateCore?) -> { run }
  // getCoreModule compiles each embedded core wasm. The transpiled
  // .core.wasm files live next to guest.js.
  // Absolute, origin-rooted base so the core filenames resolve
  // unambiguously regardless of how the bundler rewrites import.meta.url.
  const guestDir = new URL('/transpiled/guest/', location.origin)
  const getCoreModule = async (path) => {
    // `path` is the bare core filename, e.g. "guest.core.wasm".
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

  // instantiateCore: JSPI-promise the core instantiation so that any
  // async WASI import (stream blocking ops) the guest hits can suspend.
  // For an in-memory stdout sink the write path is synchronous, but
  // wrapping is harmless and matches the CLI-component pattern from
  // sqlink-composed.js (Suspending imports / promising exports under
  // Chromium's default-on JSPI).
  const instantiateCore = async (module, importObj) => {
    return WebAssembly.instantiate(module, importObj)
  }

  const root = await guestMod.instantiate(
    getCoreModule,
    imports,
    instantiateCore,
  )

  // Drive wasi:cli/run#run. Wrap in promising-tolerant await: run() may
  // return a value (0/exit) synchronously or a Promise under JSPI.
  const runResult = root.run.run()
  if (runResult && typeof runResult.then === 'function') {
    await runResult
  }

  return { stdout, stderr, stats }
}

/**
 * Demonstrate SHARING: run the guest TWICE. Both runs resolve + invoke
 * through the linker, both hit the ONE provider instance imported once.
 *
 * @returns {Promise<object>} structured evidence
 */
export async function runSharedDemo() {
  const resolves = []
  const invokes = []
  const hooks = {
    onResolve: (id) => resolves.push(id),
    onInvoke: (m, instanceId) => invokes.push({ method: m, instanceId }),
  }

  const run1 = await runGuest(hooks)
  const run2 = await runGuest(hooks)

  return {
    run1: { stdout: run1.stdout.trim(), stderr: run1.stderr.trim(), stats: run1.stats },
    run2: { stdout: run2.stdout.trim(), stderr: run2.stderr.trim(), stats: run2.stats },
    // The single, shared provider was imported (== instantiated) once
    // across BOTH runs.
    providerImportCount,
    totalResolves: resolves.length,
    totalInvokes: invokes.length,
    resolves,
    invokes,
  }
}
