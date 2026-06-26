import { defineConfig, devices } from '@playwright/test'

// JSPI: this guest is a wasi:cli/run CLI driven through
// @tegmentum/wasi-polyfill. CLI components can suspend on async WASI
// imports (wasi:io/streams blocking ops, wasi:io/poll.block), which
// requires WebAssembly.Suspending / WebAssembly.promising. Playwright's
// bundled Chromium (137+) ships JSPI ENABLED BY DEFAULT, so no
// `--js-flags=--experimental-wasm-jspi` is needed. If a CI runner ever
// pins chromium below 137, re-introduce the flag under
// `use.launchOptions.args`.
export default defineConfig({
  testDir: './tests',
  fullyParallel: false,
  workers: 1,
  timeout: 60_000,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:5181',
    trace: 'off',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: {
    command: 'npx vite --host 127.0.0.1 --port 5181 --strictPort',
    url: 'http://127.0.0.1:5181',
    reuseExistingServer: !process.env.CI,
    timeout: 90_000,
  },
})
