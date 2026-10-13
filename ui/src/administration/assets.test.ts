import { build } from 'vite'
import { expect, it } from 'vitest'

it('builds Administration Interface assets with absolute root paths', async () => {
  const result = await build({
    configFile: 'vite.config.ts',
    build: { write: false },
  })
  const bundles = Array.isArray(result) ? result : [result]
  for (const bundle of bundles) {
    if (!('output' in bundle)) throw new Error('The build does not return its assets.')
    const page = bundle.output.find((file) => file.fileName === 'administration.html')
    if (page?.type !== 'asset') throw new Error('The Administration Interface is not built.')
    const html = String(page.source)
    const assets = [...html.matchAll(/(?:src|href)="([^"]+)"/g)].map((match) => match[1])
    expect(assets.length).toBeGreaterThan(0)
    for (const asset of assets) {
      expect(asset).toMatch(/^\/assets\//)
      expect(bundle.output.some((file) => `/${file.fileName}` === asset)).toBe(true)
    }
  }
}, 60_000)
