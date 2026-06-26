// WASI Preview 2 host imports for the jco-transpiled dynlink guest,
// supplied by @tegmentum/wasi-polyfill. Mirrors sqlink's
// browser/src/host-imports.js `buildCliHostImports` (jcoCompat
// un-versioned import names for jco's async-mode
// `instantiate(getCoreModule, imports)` path), trimmed to exactly
// what THIS guest imports: wasi:cli/* + wasi:io/*.
//
// The guest is a `wasi:cli/run` CLI that resolves a provider through
// `compose:dynlink/linker`, invokes it, and `println!`s the result.
// We capture stdout via the stdout plugin's `onStdout` callback so the
// harness can read the printed `HELLO FROM DLOPEN`.

import { createPolyfill, createPolicy } from '@tegmentum/wasi-polyfill/wasip2'
import {
  randomPlugin,
  insecureRandomPlugin,
  insecureSeedPlugin,
} from '@tegmentum/wasi-polyfill/wasip2/plugins/random'
import {
  monotonicClockPlugin,
  wallClockPlugin,
} from '@tegmentum/wasi-polyfill/wasip2/plugins/clocks'
import {
  errorPlugin,
  pollPlugin,
  streamsPlugin,
} from '@tegmentum/wasi-polyfill/wasip2/plugins/io'
import {
  environmentPlugin,
  exitPlugin,
  stdinPlugin,
  stdoutPlugin,
  stderrPlugin,
  terminalInputPlugin,
  terminalOutputPlugin,
  terminalStdinPlugin,
  terminalStdoutPlugin,
  terminalStderrPlugin,
} from '@tegmentum/wasi-polyfill/wasip2/plugins/cli'

/**
 * Build the fully-resolved WASI imports object (jcoCompat / un-versioned
 * import names) for the guest's async-mode `instantiate`. stdout/stderr
 * bytes are streamed to the supplied callbacks.
 *
 * @param {{ onStdout?: (data: Uint8Array) => void, onStderr?: (data: Uint8Array) => void }} opts
 * @returns {Promise<{ imports: Record<string, unknown>, polyfill: import('@tegmentum/wasi-polyfill/wasip2').Polyfill }>}
 */
export async function buildWasiImports(opts = {}) {
  const overrides = []
  if (opts.onStdout) {
    overrides.push({
      interface: 'wasi:cli/stdout@0.2.6',
      options: { onStdout: opts.onStdout },
    })
  }
  if (opts.onStderr) {
    overrides.push({
      interface: 'wasi:cli/stderr@0.2.6',
      options: { onStderr: opts.onStderr },
    })
  }
  const policy = createPolicy({ defaultAllow: true, overrides })
  const polyfill = createPolyfill({ policy })

  polyfill.registerPlugin(randomPlugin)
  polyfill.registerPlugin(insecureRandomPlugin)
  polyfill.registerPlugin(insecureSeedPlugin)
  polyfill.registerPlugin(monotonicClockPlugin)
  polyfill.registerPlugin(wallClockPlugin)
  polyfill.registerPlugin(errorPlugin)
  polyfill.registerPlugin(pollPlugin)
  polyfill.registerPlugin(streamsPlugin)
  polyfill.registerPlugin(environmentPlugin)
  polyfill.registerPlugin(exitPlugin)
  polyfill.registerPlugin(stdinPlugin)
  polyfill.registerPlugin(stdoutPlugin)
  polyfill.registerPlugin(stderrPlugin)
  polyfill.registerPlugin(terminalInputPlugin)
  polyfill.registerPlugin(terminalOutputPlugin)
  polyfill.registerPlugin(terminalStdinPlugin)
  polyfill.registerPlugin(terminalStdoutPlugin)
  polyfill.registerPlugin(terminalStderrPlugin)

  const { imports } = await polyfill.forInterfaces(
    [
      'wasi:cli/environment@0.2.6',
      'wasi:cli/exit@0.2.6',
      'wasi:cli/stdin@0.2.6',
      'wasi:cli/stdout@0.2.6',
      'wasi:cli/stderr@0.2.6',
      'wasi:cli/terminal-input@0.2.6',
      'wasi:cli/terminal-output@0.2.6',
      'wasi:cli/terminal-stdin@0.2.6',
      'wasi:cli/terminal-stdout@0.2.6',
      'wasi:cli/terminal-stderr@0.2.6',
      'wasi:clocks/monotonic-clock@0.2.6',
      'wasi:clocks/wall-clock@0.2.6',
      'wasi:io/error@0.2.6',
      'wasi:io/poll@0.2.6',
      'wasi:io/streams@0.2.6',
      'wasi:random/insecure-seed@0.2.6',
    ],
    { jcoCompat: true, throwOnMissing: false, throwOnDenied: false },
  )

  return { imports, polyfill }
}
