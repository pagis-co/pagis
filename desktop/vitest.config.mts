/// <reference types="vitest/config" />
import { defineConfig } from 'vitest/config'

// The shell's tests drive the supervisor against a fake daemon script,
// so they need no built daemon binary and no Electron runtime.
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    testTimeout: 20000,
  },
})
