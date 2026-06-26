import { test, expect } from '@playwright/test'

// Records the engine + whether JSPI (WebAssembly.Suspending /
// WebAssembly.promising) is available WITHOUT any launch flag — the same
// default-on situation the dynlink CLI guest relies on.
test('JSPI is available in the test browser', async ({ page, browser }) => {
  await page.goto('/')
  const probe = await page.evaluate(() => ({
    hasSuspending: typeof WebAssembly.Suspending === 'function',
    hasPromising: typeof WebAssembly.promising === 'function',
    userAgent: navigator.userAgent,
  }))
  console.log('engine:', browser.browserType().name(), browser.version())
  console.log('jspi probe:', JSON.stringify(probe))
  expect(probe.hasSuspending).toBe(true)
  expect(probe.hasPromising).toBe(true)
})
