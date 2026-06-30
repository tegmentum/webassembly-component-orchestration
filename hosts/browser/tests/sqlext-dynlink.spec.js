import { test, expect } from '@playwright/test'

// Task #219 — browser arm. Proves the SAME compose:dynlink machinery
// loads + dispatches REAL sqlink declarative sqlite:extension tiers
// (scalar / aggregate / collation / vtab) through the REUSABLE
// sqlite-extension-endpoint provider, in a headless browser: a
// jco-transpiled generic harness resolves each composed provider through
// a JS compose:dynlink/linker, sends CBOR envelopes (describe /
// policy-check / per-tier dispatch), and prints correct results.
test('declarative sqlite:extension tiers load + dispatch via compose:dynlink in the browser', async ({
  page,
}) => {
  page.on('pageerror', (e) => console.error('[pageerror]', e))
  page.on('console', (msg) => {
    if (msg.type() === 'error') console.error('[console.error]', msg.text())
  })

  await page.goto('/sqlext.html')
  await page.waitForFunction(() => window.__sqlextDone === true, { timeout: 120_000 })

  const error = await page.evaluate(() => window.__sqlextError)
  expect(error, error || 'no error').toBeUndefined()

  const r = await page.evaluate(() => window.__sqlextResult)
  console.log(JSON.stringify(r, null, 2))

  // scalar (aba)
  expect(r.scalar.stdout).toContain('loaded extension: aba')
  expect(r.scalar.stdout).toContain("aba_validate('021000021') => 1")
  expect(r.scalar.stdout).toContain("aba_validate('021000022') => 0")

  // aggregate (count_min): full step+finalize lifecycle, then estimate.
  expect(r.aggregate.stdout).toContain('loaded extension: count_min')
  expect(r.aggregate.stdout).toContain("count_min_estimate(sketch, 'apple') => 3")
  expect(r.aggregate.stdout).toContain("count_min_estimate(sketch, 'durian') => 0")

  // collation (uint): natural-numeric order.
  expect(r.collation.stdout).toContain('loaded extension: uint')
  expect(r.collation.stdout).toContain("('x2' < 'x10')")

  // vtab (series): full read-cursor surface.
  expect(r.vtab.stdout).toContain('loaded extension: series')
  expect(r.vtab.stdout).toContain('generate_series(1,5) => [1, 2, 3, 4, 5]')

  // every tier reconciled policy (fail-closed gate) and ran describe.
  for (const tier of ['scalar', 'aggregate', 'collation', 'vtab']) {
    expect(r[tier].stdout).toContain('policy-check: ok=true')
    expect(r[tier].invokes).toContain('describe')
    expect(r[tier].invokes).toContain('policy-check')
    expect(r[tier].providerImportCount).toBe(1)
  }
})
