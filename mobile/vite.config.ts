/// <reference types="vitest/config" />
import { defineConfig } from 'vite'

// The app bundles the Connect screen alone. `npx cap sync` copies the
// build in `dist/` into the native projects.
export default defineConfig({
  // The bundled page loads from `capacitor://localhost` on iOS and from
  // `https://localhost` on Android, so its assets have relative paths.
  base: './',
  test: { environment: 'jsdom' },
})
