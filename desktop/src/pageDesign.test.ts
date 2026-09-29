// The Client App's own pages, setup and status, use the design system of
// the Product App (docs/UI-DESIGN.md). The build copies the tokens, the
// Inter font and the Pagis mark into dist/design from their one source,
// so the pages load from the app package with no network, and no value
// is copied by hand.

import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

const desktop = path.join(__dirname, '..')
const repository = path.join(desktop, '..')
const PAGES = ['setup.html', 'status.html']

const roots: string[] = []
afterEach(() => { while (roots.length > 0) fs.rmSync(roots.pop()!, { recursive: true, force: true }) })

function read(file: string): string {
  return fs.readFileSync(path.join(desktop, file), 'utf8')
}

/** The declarations of the one rule whose selector is `selector`. */
function rule(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const match = new RegExp(`(?:^|\\})\\s*${escaped}\\s*\\{([^}]*)\\}`).exec(css)
  if (!match) throw new Error(`pages.css has no rule for ${selector}`)
  return match[1]
}

function withoutComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, '')
}

describe('the design of the Client App pages', () => {
  it('copies the tokens, the font and the mark from their sources in the build', () => {
    const output = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-design-'))
    roots.push(output)

    execFileSync(process.execPath, [path.join(desktop, 'scripts', 'copy-design.mjs'), output])

    const copied: Record<string, string> = {
      'tokens.css': path.join(repository, 'ui', 'src', 'tokens.css'),
      'inter.woff2': path.join(desktop, 'node_modules', '@fontsource-variable', 'inter', 'files', 'inter-latin-wght-normal.woff2'),
      'pagis-mark.svg': path.join(repository, 'assets', 'brand', 'pagis-mark.svg'),
      'pagis-mark-dark.svg': path.join(repository, 'assets', 'brand', 'pagis-mark-dark.svg'),
    }
    expect(fs.readdirSync(output).sort()).toEqual(Object.keys(copied).sort())
    for (const [name, source] of Object.entries(copied)) {
      expect(fs.readFileSync(path.join(output, name)).equals(fs.readFileSync(source)), name).toBe(true)
    }
  })

  it('loads the tokens, the page stylesheet and the mark from the package alone', () => {
    for (const page of PAGES) {
      const html = read(`static/${page}`)
      const policy = /http-equiv="Content-Security-Policy" content="([^"]+)"/.exec(html)?.[1] ?? ''

      expect(html, page).toContain('<link rel="stylesheet" href="../dist/design/tokens.css" />')
      expect(html, page).toContain('<link rel="stylesheet" href="pages.css" />')
      expect(html, page).toContain('src="../dist/design/pagis-mark.svg"')
      expect(html, page).toContain('srcset="../dist/design/pagis-mark-dark.svg"')
      expect(policy, page).toMatch(/default-src 'none'/)
      expect(policy, page).not.toMatch(/https?:|\*/)
      expect(policy, page).not.toMatch(/style-src[^;]*'unsafe-inline'/)
      expect(html, page).not.toMatch(/<style|\sstyle="/)
    }
  })

  it('reads every color, size, radius and shadow of the page stylesheet from the tokens', () => {
    // The one @font-face names the weight range of the font file itself.
    const css = withoutComments(read('static/pages.css')).replace(/@font-face\s*\{[^}]*\}/g, '')
    const tokens = fs.readFileSync(path.join(repository, 'ui', 'src', 'tokens.css'), 'utf8')

    expect(css).not.toMatch(/#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?)\s*\(/)
    for (const property of ['font-size', 'font-weight', 'line-height', 'border-radius', 'box-shadow', 'padding', 'margin', 'gap']) {
      const values = [...css.matchAll(new RegExp(`(?<![-\\w])${property}\\s*:([^;{}]*)`, 'g'))].map((match) => match[1].trim())
      const literal = values.filter((value) => value.replace(/var\(--[\w-]+\)/g, '').replace(/\b(0|auto)\b/g, '').trim() !== '')
      expect(literal, property).toEqual([])
    }
    for (const [, name] of css.matchAll(/var\((--[\w-]+)\)/g)) {
      expect(tokens, `${name} is not a token`).toContain(`${name}:`)
    }
  })

  // The window has its own frame, so the page draws no second one: the
  // content sits on the ground of the window, as the first run of an
  // established desktop app does.
  it('puts the content on the ground of the window, with no card frame', () => {
    const css = withoutComments(read('static/pages.css'))
    const page = rule(css, '.page')

    expect(css).not.toMatch(/\.card\b/)
    for (const property of ['background', 'border', 'border-radius', 'box-shadow']) {
      expect(page, property).not.toMatch(new RegExp(`(?<![-\\w])${property}\\s*:`))
    }
    for (const file of PAGES) {
      expect(read(`static/${file}`), file).not.toMatch(/class="card"/)
    }
  })

  // On macOS the window buttons sit over the top of the page. A strip as
  // high as their area moves the window and keeps the content under it.
  // A browser defines no title bar area, so the strip has no height there.
  it('gives the window buttons their area and a drag region at the top', () => {
    const titlebar = rule(withoutComments(read('static/pages.css')), '.titlebar')

    expect(titlebar).toMatch(/(?<![-\w])height:\s*env\(titlebar-area-height, 0px\)/)
    expect(titlebar).toMatch(/-webkit-app-region:\s*drag/)
    expect(titlebar).toMatch(/position:\s*sticky/)
    for (const file of PAGES) {
      expect(read(`static/${file}`), file).toMatch(/<body>\s*<div class="titlebar" aria-hidden="true"><\/div>\s*<main class="page">/)
    }
  })

  // The setup is a short flow of screens in a window of fixed size. The
  // footer with Quit and the next step stays at the bottom edge, and the
  // content above it takes the rest of the window. A state longer than
  // the window, such as a long failure, scrolls in the content alone.
  it('pins the footer to the bottom edge of the window, and scrolls only the content', () => {
    const css = withoutComments(read('static/pages.css'))

    expect(rule(css, 'html,\nbody')).toMatch(/(?<![-\w])height:\s*100%/)
    expect(rule(css, 'body')).toMatch(/flex-direction:\s*column/)
    expect(rule(css, '.page')).toMatch(/(?<![-\w])flex:\s*1/)
    expect(rule(css, '.page')).toMatch(/min-height:\s*0/)
    expect(rule(css, '.content')).toMatch(/(?<![-\w])flex:\s*1/)
    expect(rule(css, '.content')).toMatch(/min-height:\s*0/)
    expect(rule(css, '.content')).toMatch(/overflow-y:\s*auto/)
    expect(rule(css, '.footer')).toMatch(/(?<![-\w])flex:\s*none/)
    for (const file of PAGES) {
      expect(read(`static/${file}`), file).toMatch(/<div class="footer">[\s\S]*<\/div>\s*<\/main>/)
    }
  })

  // As the first-run windows of 1Password, Raycast and Linear: the
  // column sits in the middle of the area between the title bar strip
  // and the footer, and the spare height splits above and below it. Auto
  // margins, and not `justify-content: center`, put a column that is
  // higher than the area at its top, so the top never clips.
  it('centres the column in the content area, and starts a taller column at the top', () => {
    const css = withoutComments(read('static/pages.css'))
    const content = rule(css, '.content')
    const column = rule(css, '.column')

    expect(content).toMatch(/display:\s*flex/)
    expect(content).toMatch(/flex-direction:\s*column/)
    expect(content).not.toMatch(/justify-content/)
    expect(column).toMatch(/margin-block:\s*auto/)
    expect(column).toMatch(/margin-inline:\s*auto/)
    for (const file of PAGES) {
      expect(read(`static/${file}`), file).toMatch(/<div class="content">\s*<div class="column">/)
    }
  })

  // The footer is a part of the page, not a separate bar.
  it('draws no line above the footer', () => {
    expect(rule(withoutComments(read('static/pages.css')), '.footer')).not.toMatch(/border/)
  })
})
