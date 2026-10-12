// The build of the service worker. A browser loads `/sw.js` as a classic
// script, which cannot hold an `import` statement, so the build writes
// one self-contained file at the root of `dist`. It also writes the
// license list of the npm packages in that file, for the notices.

import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build, type Rolldown } from 'vite'
import { describe, expect, it } from 'vitest'

const uiDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')

describe('the service worker build', () => {
  it('writes sw.js, with no import statement, and its license list', async () => {
    const result = await build({
      root: uiDir,
      configFile: resolve(uiDir, 'vite.sw.config.ts'),
      logLevel: 'silent',
      build: { write: false },
    })
    const outputs = (Array.isArray(result) ? result : [result]) as Rolldown.RolldownOutput[]
    const files = outputs.flatMap((output) => output.output)
    expect(files.map((file) => file.fileName)).toEqual(['sw.js', '.vite/license-sw.json'])
    const [worker] = files
    expect(worker.type).toBe('chunk')
    const code = worker.type === 'chunk' ? worker.code : ''
    expect(code).toContain('skipWaiting')
    expect(code).not.toMatch(/\bimport\b/)
  })
})
