// Scrolling. A column that scrolls down must not also scroll
// sideways. The browser gives an element whose `overflow-y` is `auto`
// a used `overflow-x` of `auto` as well, so one wide child — a code
// block, a table — drags the whole column under the reader's hands and
// puts a second scrollbar across the foot of the panel. The column
// says which way it scrolls, and content that is genuinely wider than
// the column carries its own scroller.

import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const srcDir = resolve(dirname(fileURLToPath(import.meta.url)))

/** Every source file of the app, as a path under `src`. */
function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return sources(path)
    if (!/\.tsx?$/.test(entry.name) || entry.name.includes('.test.')) return []
    return [relative(srcDir, path)]
  })
}

function stylesheets(dir: string): { name: string; css: string }[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return stylesheets(path)
    if (!entry.name.endsWith('.css')) return []
    return [{ name: relative(srcDir, path), css: readFileSync(path, 'utf8') }]
  })
}

describe('a column that scrolls down does not scroll sideways', () => {
  it('writes no lone overflow-y', () => {
    for (const sheet of stylesheets(srcDir)) {
      expect(
        /overflow-y\s*:/.test(sheet.css),
        `${sheet.name} sets overflow-y alone, which lets the column scroll sideways too; write "overflow: hidden auto"`,
      ).toBe(false)
    }
  })

  // The timeline's scroller belongs to the virtualizer, which writes
  // `overflow-y` inline. Only the stylesheet can close the other axis.
  it('closes the sideways axis of the timeline', () => {
    const css = readFileSync(join(srcDir, 'components/Timeline.css'), 'utf8')
    expect(/\.timeline\s*\{[^}]*overflow-x:\s*hidden/.test(css)).toBe(true)
  })
})

describe('the scrollbar is a hairline', () => {
  // `scrollbar-color` is inherited and `scrollbar-width` is not, so
  // the colour is set once and the width is worn by every element.
  it('sets a thin bar in the border colour', () => {
    const css = readFileSync(join(srcDir, 'styles.css'), 'utf8')
    expect(/html\s*\{[^}]*scrollbar-color:/.test(css)).toBe(true)
    expect(/\*\s*\{[^}]*scrollbar-width:\s*thin/.test(css)).toBe(true)
  })
})

describe('prose carries its own scroller for what cannot wrap', () => {
  const css = readFileSync(join(srcDir, 'prose.css'), 'utf8')

  it('gives a code block a scroller of its own', () => {
    expect(/\.prose\s+pre\s*\{[^}]*overflow-x:\s*auto/.test(css)).toBe(true)
  })

  it('breaks a word too long for the column', () => {
    expect(/\.prose\s*\{[^}]*overflow-wrap:\s*anywhere/.test(css)).toBe(true)
  })

  it('gives a Markdown table a scroller of its own', () => {
    expect(/\.prose-table\s*\{[^}]*overflow-x:\s*auto/.test(css)).toBe(true)
  })

  it('is worn by every Markdown wrapper', () => {
    for (const file of ['blocks/BlockView.tsx', 'components/memory/PageView.tsx']) {
      const code = readFileSync(join(srcDir, file), 'utf8')
      const wrappers = [...code.matchAll(/<Prose>/g)]
      expect(wrappers.length, `${file} renders no Markdown`).toBeGreaterThan(0)
      expect(/className="[^"]*\bprose\b/.test(code), `${file} wraps Markdown without the prose class`).toBe(true)
    }
  })

  // One renderer holds the extensions and the scrollers, so a second
  // call site would render Markdown under neither.
  it('is the only renderer of Markdown', () => {
    for (const file of sources(srcDir)) {
      const code = readFileSync(join(srcDir, file), 'utf8')
      expect(
        code.includes('react-markdown'),
        `${file} renders Markdown of its own instead of using Prose`,
      ).toBe(file === 'prose.tsx')
    }
  })
})
