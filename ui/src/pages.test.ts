// The two entry pages. The browser tab, and the window of the Client
// App, show the title of the page, so each page names the product.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

import { themes, value } from './test/tokens'

const uiDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')

function title(page: string): string | undefined {
  const html = readFileSync(resolve(uiDir, page), 'utf8')
  return /<title>([^<]*)<\/title>/.exec(html)?.[1]
}

describe('the entry pages', () => {
  it('name the product Pagis', () => {
    expect(title('index.html')).toBe('Pagis')
    expect(title('administration.html')).toBe('Administration · Pagis')
  })
})

// The Product App on a phone. The page draws under the notch and the
// home indicator, the on-screen keyboard shrinks the layout, and the
// browser paints its bars in the ground of the system theme.
describe('the product page on a phone', () => {
  const page = new DOMParser().parseFromString(
    readFileSync(resolve(uiDir, 'index.html'), 'utf8'),
    'text/html',
  )

  it('covers the whole screen and resizes for the keyboard', () => {
    expect(page.querySelector('meta[name="viewport"]')?.getAttribute('content')).toBe(
      'width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content',
    )
  })

  it('gives the browser bars the ground of each system theme', () => {
    const metas = [...page.querySelectorAll('meta[name="theme-color"]')].map((meta) => ({
      media: meta.getAttribute('media'),
      content: meta.getAttribute('content'),
    }))
    expect(metas).toEqual(
      themes().map((theme) => ({
        media: `(prefers-color-scheme: ${theme.name})`,
        content: value(theme.tokens, '--ground'),
      })),
    )
  })
})
