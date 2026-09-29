/// <reference types="vitest/config" />
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'
import { fileURLToPath } from 'node:url'

// The daemon serves the built bundle itself; the dev server exists only
// for the edit loop and proxies the API to a locally running daemon.
const apiTarget = process.env.PAGIS_DEV_API_URL ?? 'http://127.0.0.1:4400'

export default defineConfig({
  plugins: [react()],
  // Two entry points, one package: the product page and the
  // administration page. The daemon serves each one on its own port and
  // both share the built assets, so every component they have in common
  // is one component.
  build: {
    rollupOptions: {
      input: {
        index: fileURLToPath(new URL('index.html', import.meta.url)),
        administration: fileURLToPath(
          new URL('administration.html', import.meta.url),
        ),
      },
    },
  },
  server: {
    fs: { allow: [fileURLToPath(new URL('..', import.meta.url))] },
    proxy: {
      '/api': {
        target: apiTarget,
        ws: true,
        // Each listener of the daemon refuses a socket upgrade from an
        // origin it does not serve, and the page of the dev server is
        // another origin. A browser sends no `Sec-Fetch-Site` on a
        // WebSocket handshake, so the proxy gives the handshake the
        // origin of the daemon. A fetch passes on
        // `Sec-Fetch-Site: same-origin` and needs no change.
        configure: (proxy) => {
          proxy.on('proxyReqWs', (proxyReq) => {
            proxyReq.setHeader('origin', new URL(apiTarget).origin)
          })
        },
      },
    },
  },
  test: {
    environment: 'jsdom',
    // Testing-library registers its afterEach cleanup via the globals.
    globals: true,
    // The Radix overlays call into pointer capture and layout, which
    // jsdom does not carry.
    setupFiles: ['./src/test-setup.ts'],
    // A test that walks a component through several `findBy` waits
    // spends up to one second of wall clock in each of them, and the
    // merge gate compiles Rust while these run. The default five
    // seconds is a budget for an idle machine. Each `findBy` keeps its
    // own one-second limit, so a real regression still fails fast; this
    // only stops a loaded runner from reading as a failure.
    testTimeout: 20_000,
  },
})
