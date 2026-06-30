import { test, expect } from '@playwright/test'

// SPIKE #218 — browser arm. Proves the SAME compose:dynlink machinery
// loads + dispatches a REAL sqlink sqlite:extension scalar extension (aba),
// modeled as a compose:dynlink/endpoint resident provider, in a headless
// browser: a jco-transpiled flavor-B dlopen guest resolves the aba
// provider through a JS compose:dynlink/linker, sends CBOR describe/call
// envelopes over the provider's endpoint, and prints the scalar results.
test('aba scalar extension loads + dispatches via compose:dynlink in the browser', async ({
  page,
}) => {
  page.on('pageerror', (e) => console.error('[pageerror]', e))
  page.on('console', (msg) => {
    if (msg.type() === 'error') console.error('[console.error]', msg.text())
    else if (msg.type() === 'warning') console.warn('[console.warn]', msg.text())
  })

  await page.goto('/aba.html')
  await page.waitForFunction(() => window.__abaDone === true, {
    timeout: 60_000,
  })

  const error = await page.evaluate(() => window.__abaError)
  expect(error, error || 'no error').toBeUndefined()

  const result = await page.evaluate(() => window.__abaResult)
  console.log(JSON.stringify(result, null, 2))

  // 1. describe() round-tripped through the provider endpoint and surfaced
  //    the registered scalar table.
  expect(result.stdout).toContain('loaded extension: aba')
  expect(result.stdout).toContain('aba_validate')

  // 2. The scalar dispatch (call) returned the correct answers — proving
  //    the aba sqlite:extension actually executed behind the provider.
  expect(result.stdout).toContain("aba_validate('021000021') => 1")
  expect(result.stdout).toContain("aba_validate('021000022') => 0")

  // 3. The provider (aba-endpoint + aba) was instantiated exactly once and
  //    driven through the linker (resolve + invokes).
  expect(result.providerImportCount).toBe(1)
  expect(result.resolves).toContain('aba')
  expect(result.invokes).toContain('describe')
  expect(result.invokes).toContain('call')

  // 4. DOM reflects it for a human visiting the page.
  const domText = await page.locator('#out').textContent()
  expect(domText).toContain("aba_validate('021000021') => 1")
})
