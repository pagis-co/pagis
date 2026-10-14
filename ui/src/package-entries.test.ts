import {
  copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, symlinkSync, writeFileSync,
} from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build, createServer } from 'vite'
import { describe, expect, it } from 'vitest'

import { stripComments, themes, value } from './test/tokens'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const primitives = join(root, 'src/primitives')
const fixture = join(root, 'src/test/package-entry/main.ts')
const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'))

function dependency(id: string): boolean {
  return Object.keys(manifest.dependencies).some(
    (name) => id === name || id.startsWith(`${name}/`),
  )
}

describe('the UI package entries', () => {
  it('keeps the full primitives module graph inside the primitives and listed dependencies', async () => {
    const modules = new Set<string>()
    await build({
      root,
      configFile: false,
      logLevel: 'silent',
      plugins: [
        {
          name: 'inspect-primitives',
          moduleParsed(module) {
            modules.add(module.id)
          },
        },
      ],
      build: {
        write: false,
        minify: false,
        rollupOptions: {
          input: 'pagis-ui/primitives',
          external: dependency,
          treeshake: false,
        },
      },
    })
    expect(modules.size).toBeGreaterThan(1)
    expect([...modules].filter((id) => !id.startsWith(`${primitives}/`))).toEqual([])
  })

  it('exports the shell components with no API client, query or app state in the full module graph', async () => {
    const modules = new Set<string>()
    const result = await build({
      root,
      configFile: false,
      logLevel: 'silent',
      plugins: [{
        name: 'inspect-shell',
        moduleParsed(module) { modules.add(module.id) },
      }],
      build: {
        write: false,
        minify: false,
        rollupOptions: {
          input: 'pagis-ui/shell',
          external: dependency,
          treeshake: false,
          preserveEntrySignatures: 'strict',
        },
      },
    })
    expect(modules.size).toBeGreaterThan(1)
    expect([...modules].filter((id) => !id.startsWith(`${root}/src/`))).toEqual([])
    expect([...modules].filter((id) => /\/api\/client|\/queries(?:[./])|\/state(?:[./])/.test(id))).toEqual([])
    if (Array.isArray(result) || !('output' in result)) throw new Error('expected one build')
    const entry = result.output.find((item) => item.type === 'chunk' && item.isEntry)
    expect(entry?.type === 'chunk' && entry.exports).toEqual(['LargeTitle', 'NavBar', 'SettingsNav', 'SidebarFrame'])
  })

  it('builds another app with the base layer, the shell components, prop types and styles', async () => {
    const app = mkdtempSync(join(root, '.package-entry-'))
    copyFileSync(join(root, 'src/test/package-entry/shell.ts'), join(app, 'main.ts'))
    writeFileSync(join(app, 'package.json'), JSON.stringify({ name: 'shell-app', type: 'module' }))
    mkdirSync(join(app, 'node_modules'))
    symlinkSync(root, join(app, 'node_modules/pagis-ui'), 'dir')
    try {
      const result = await build({
        root,
        configFile: false,
        logLevel: 'silent',
        build: { write: false, rollupOptions: { input: join(app, 'main.ts') } },
      })
      if (Array.isArray(result) || !('output' in result)) throw new Error('expected one build')
      const css = result.output.flatMap((item) =>
        item.type === 'asset' && item.fileName.endsWith('.css') ? [String(item.source)] : [],
      ).join('\n')
      // The base layer sets the box model that the shell sizes depend on.
      expect(css).toMatch(/\*\s*\{[^}]*box-sizing:\s*border-box/)
      for (const selector of ['.sidebar', '.org-head', '.person-notice', '.settings-nav-item', '.org-settings-tunnel', '.nav-bar', '.large-title']) {
        expect(css).toContain(selector)
      }
    } finally {
      rmSync(app, { recursive: true, force: true })
    }
  })

  it('defines every property the primitives read in both themes', () => {
    const sheets = readdirSync(primitives).filter((name) => name.endsWith('.css'))
    const reads = sheets.flatMap((name) => {
      const css = stripComments(readFileSync(join(primitives, name), 'utf8'))
      return [...css.matchAll(/var\((--[\w-]+)/g)].map((match) => match[1])
    })
    for (const theme of themes()) {
      // Radix supplies these measured lengths at run time.
      const measured = new Set([
        '--radix-popover-trigger-width',
        '--radix-popover-content-available-height',
      ])
      const tokens = new Set(reads.filter((name) => !measured.has(name)))
      for (const token of tokens) {
        const definition = theme.tokens[token]
        expect(definition, `${theme.name}: ${token}`).toBeDefined()
        for (const [, alias] of definition.matchAll(/var\((--[\w-]+)/g)) {
          tokens.add(alias)
        }
      }
    }
  })

  it('builds and renders another app with only the exported styles and fonts', async () => {
    const app = mkdtempSync(join(root, '.package-entry-'))
    const entry = join(app, 'main.ts')
    copyFileSync(fixture, entry)
    writeFileSync(
      join(app, 'package.json'),
      JSON.stringify({ name: 'fixture-app', type: 'module' }),
    )
    mkdirSync(join(app, 'node_modules'))
    symlinkSync(root, join(app, 'node_modules/pagis-ui'), 'dir')
    try {
      const result = await build({
        root,
        configFile: false,
        logLevel: 'silent',
        build: { write: false, rollupOptions: { input: entry } },
      })
      if (Array.isArray(result) || !('output' in result)) throw new Error('expected one build')
      const css = result.output.flatMap((entry) =>
        entry.type === 'asset' && entry.fileName.endsWith('.css')
          ? [String(entry.source)] : [],
      ).join('\n')
      expect(css).toContain('@font-face')
      expect(css).toContain('Inter Variable')
      expect(css).toContain('--accent')
      const server = await createServer({
        root,
        configFile: false,
        ssr: { noExternal: ['pagis-ui'] },
        server: { middlewareMode: true, hmr: false, ws: false },
      })
      try {
        const { html } = await server.ssrLoadModule(entry)
        for (const theme of themes()) {
          // Resolve the tokens before jsdom reads the CSS. It does not resolve var().
          const resolved = css.replace(/var\((--[\w-]+)\)/g, (match, token: string) =>
            theme.tokens[token] === undefined ? match : value(theme.tokens, token),
          )
          document.body.innerHTML = `<style>${resolved}</style>${html}`
          const button = document.querySelector('button')!
          const frame = document.querySelector('.ui-frame')!
          expect(button.textContent).toBe('Continue')
          expect(frame.contains(button)).toBe(true)
          const color = (token: string) => {
            const probe = document.createElement('span')
            probe.style.color = value(theme.tokens, token)
            return probe.style.color
          }
          expect(getComputedStyle(button).backgroundColor).toBe(color('--accent'))
          expect(getComputedStyle(button).color).toBe(color('--on-accent'))
          expect(getComputedStyle(frame).backgroundColor).toBe(color('--raised'))
          document.body.innerHTML = ''
        }
      } finally {
        await server.close()
      }
    } finally {
      rmSync(app, { recursive: true, force: true })
    }
  })
})
