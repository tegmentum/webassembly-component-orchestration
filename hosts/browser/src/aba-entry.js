// Browser entry for the aba-dynlink spike arm: loads + dispatches the aba
// scalar extension through compose:dynlink and surfaces the result to the
// DOM + a global the Playwright spec polls.

import { runAbaDemo } from './aba-host.js'

const statusEl = document.getElementById('status')
const outEl = document.getElementById('out')

function log(s) {
  if (outEl) outEl.textContent += s + '\n'
  // eslint-disable-next-line no-console
  console.log(s)
}

async function main() {
  if (statusEl) statusEl.textContent = 'running aba compose:dynlink browser host...'
  const result = await runAbaDemo()

  log('--- guest stdout ---')
  log(result.stdout)
  if (result.stderr) {
    log('--- guest stderr ---')
    log(result.stderr)
  }
  log('provider instantiated (imported) ' + result.providerImportCount + ' time(s)')
  log('resolves: ' + JSON.stringify(result.resolves))
  log('invokes: ' + JSON.stringify(result.invokes))

  window.__abaResult = result
  window.__abaDone = true
  if (statusEl) statusEl.textContent = 'done.'
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error('[aba-host] failed:', err)
  window.__abaError = String((err && err.stack) || err)
  window.__abaDone = true
  if (statusEl) statusEl.textContent = 'error: ' + err
})
