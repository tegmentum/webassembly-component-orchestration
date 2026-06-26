import { defineConfig } from 'vite'

// The transpiled provider/guest import @bytecodealliance/preview2-shim
// (provider) and reference embedded .core.wasm assets. We serve them as
// static files from transpiled/ and let Vite optimise deps off.
export default defineConfig({
  server: {
    fs: {
      // allow serving the file:-linked wasi-polyfill from outside root
      allow: ['..', '../../..', '../../../..'],
    },
  },
  optimizeDeps: {
    // jco-transpiled output + preview2-shim are ESM with top-level await
    // and embedded wasm; don't pre-bundle them. The polyfill ships a
    // nested @bytecodealliance/jco (for its runtime-bindgen path, which
    // we do NOT use) that esbuild chokes on if it tries to crawl it.
    exclude: [
      '@bytecodealliance/preview2-shim',
      '@tegmentum/wasi-polyfill',
      '@bytecodealliance/jco',
    ],
  },
  build: {
    target: 'esnext',
  },
})
