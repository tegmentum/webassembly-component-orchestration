// Browser entry for task #219: runs every declarative tier through the
// reusable sqlite-extension-endpoint provider and surfaces the results to
// the DOM + a global the Playwright spec polls.

import { runAllTiers } from './sqlext-host.js'

const statusEl = document.getElementById('status')
const outEl = document.getElementById('out')

function log(s) {
  if (outEl) outEl.textContent += s + '\n'
  // eslint-disable-next-line no-console
  console.log(s)
}

async function main() {
  if (statusEl) statusEl.textContent = 'running sqlite-extension-endpoint tiers...'
  const results = await runAllTiers()
  for (const [tier, r] of Object.entries(results)) {
    log(`--- tier ${tier} ---`)
    log(r.stdout)
    if (r.stderr) log(`[stderr] ${r.stderr}`)
  }
  window.__sqlextResult = results
  window.__sqlextDone = true
  if (statusEl) statusEl.textContent = 'done.'
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error('[sqlext-host] failed:', err)
  window.__sqlextError = String((err && err.stack) || err)
  window.__sqlextDone = true
  if (statusEl) statusEl.textContent = 'error: ' + err
})
