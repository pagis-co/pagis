/// <reference types="vitest/config" />
import { defineConfig } from 'vite'

// The app bundles two pages: the Connect screen, and the Unreachable
// screen that the shell opens when the stored server does not load.
// `npx cap sync` copies the build in `dist/` into the native projects.
export default defineConfig({
  // The bundled page loads from `capacitor://localhost` on iOS and from
  // `https://localhost` on Android, so its assets have relative paths.
  base: './',
  build: {
    rolldownOptions: {
      input: { connect: 'index.html', unreachable: 'unreachable.html' },
    },
  },
  test: { environment: 'jsdom' },
})
