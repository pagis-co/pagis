// The two entry pages. The browser tab, and the window of the Client
// App, show the title of the page, so each page names the product.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

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
