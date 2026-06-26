// Browser entry: runs the shared-provider dynlink demo and writes the
// result to the DOM + a global the Playwright spec polls.

import { runSharedDemo } from './dynlink-host.js'

const statusEl = document.getElementById('status')
const outEl = document.getElementById('out')

function log(s) {
  if (outEl) outEl.textContent += s + '\n'
  // eslint-disable-next-line no-console
  console.log(s)
}

async function main() {
  if (statusEl) statusEl.textContent = 'running compose:dynlink browser host...'
  const result = await runSharedDemo()

  log('run1 stdout: ' + JSON.stringify(result.run1.stdout))
  log('run2 stdout: ' + JSON.stringify(result.run2.stdout))
  log('provider instantiated (imported) ' + result.providerImportCount + ' time(s)')
  log('total resolves across both runs: ' + result.totalResolves)
  log('total invokes across both runs: ' + result.totalInvokes)

  window.__dynlinkResult = result
  window.__dynlinkDone = true
  if (statusEl) statusEl.textContent = 'done.'
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error('[dynlink-host] failed:', err)
  window.__dynlinkError = String((err && err.stack) || err)
  window.__dynlinkDone = true
  if (statusEl) statusEl.textContent = 'error: ' + err
})
