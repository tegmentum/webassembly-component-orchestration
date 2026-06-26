import { test, expect } from '@playwright/test'

// Proves compose:dynlink works in a real (headless) browser: a
// jco-transpiled guest resolves a provider through a JS-implemented
// compose:dynlink/linker and invokes it under WASI Preview 2 via
// @tegmentum/wasi-polyfill. The provider is instantiated ONCE and shared
// across resolves / runs.
test('compose:dynlink resolves + invokes a shared provider in the browser', async ({
  page,
}) => {
  page.on('pageerror', (e) => console.error('[pageerror]', e))
  page.on('console', (msg) => {
    if (msg.type() === 'error') console.error('[console.error]', msg.text())
    else if (msg.type() === 'warning') console.warn('[console.warn]', msg.text())
  })

  await page.goto('/')
  await page.waitForFunction(() => window.__dynlinkDone === true, {
    timeout: 60_000,
  })

  const error = await page.evaluate(() => window.__dynlinkError)
  expect(error, error || 'no error').toBeUndefined()

  const result = await page.evaluate(() => window.__dynlinkResult)
  console.log(JSON.stringify(result, null, 2))

  // 1. The guest's printed output is the uppercased echo — produced by
  //    resolving + invoking the provider through compose:dynlink/linker.
  expect(result.run1.stdout).toBe('HELLO FROM DLOPEN')
  expect(result.run2.stdout).toBe('HELLO FROM DLOPEN')

  // 2. Shared-provider evidence: the provider component was instantiated
  //    (imported) exactly ONCE across BOTH guest runs...
  expect(result.providerImportCount).toBe(1)

  // ...yet both runs resolved AND invoked through the linker (>= 2 each:
  //    one resolve + one invoke per run, both hitting the one provider).
  expect(result.totalResolves).toBeGreaterThanOrEqual(2)
  expect(result.totalInvokes).toBeGreaterThanOrEqual(2)

  // 3. The DOM reflects the result for a human visiting the page.
  const domText = await page.locator('#out').textContent()
  expect(domText).toContain('HELLO FROM DLOPEN')
  expect(domText).toContain('provider instantiated (imported) 1 time(s)')
})
