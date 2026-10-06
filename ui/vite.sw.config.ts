import { defineConfig } from 'vite'
import { fileURLToPath } from 'node:url'

// The service worker, `sw/sw.ts`, built as one classic script at the root
// of `dist`: `/sw.js` with no hash in its name, so its address stays the
// same from build to build. The build of `vite.config.ts` runs first and
// fills `dist`, so this build keeps the files there and copies no
// `public/` again. vite-plugin-pwa builds a custom worker the same way.
export default defineConfig({
  publicDir: false,
  build: {
    emptyOutDir: false,
    lib: {
      entry: fileURLToPath(new URL('sw/sw.ts', import.meta.url)),
      // An `iife` build names its global. The worker exports nothing.
      name: 'pagisServiceWorker',
      formats: ['iife'],
      fileName: () => 'sw.js',
    },
  },
})
