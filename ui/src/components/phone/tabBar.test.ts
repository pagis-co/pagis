// The tab bar of the phone layout. It follows the iOS tab bar and the
// Material 3 navigation bar: four places of equal width, an icon over
// a one-line label, the current place in the accent and the other
// places muted, and one bottom safe-area inset under the bar.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { blocks } from '../../test/css'
import { stripComments } from '../../test/tokens'

const css = stripComments(
  readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'phone.css'), 'utf8'),
)

describe('the tab bar', () => {
  // A grid track of `1fr` grows to fit its widest content, so a long
  // label would make its place wider than the others.
  it('gives each place the same width, whatever its label', () => {
    expect(blocks(css, '.tab-bar')).toMatch(/grid-template-columns:\s*repeat\(4,\s*minmax\(0,\s*1fr\)\)/)
  })

  it('keeps each label on one line', () => {
    const label = blocks(css, '.tab-bar-label')
    expect(label).toMatch(/white-space:\s*nowrap/)
    expect(label).toMatch(/overflow:\s*hidden/)
    expect(label).toMatch(/text-overflow:\s*ellipsis/)
  })

  it('mutes each place that is not current', () => {
    expect(blocks(css, '.tab-bar-place')).toMatch(/color:\s*var\(--muted\)/)
    expect(blocks(css, ".tab-bar-place[aria-current='page']")).toMatch(/color:\s*var\(--accent\)/)
  })

  // On a phone with a home indicator, the inset is the space under the
  // bar. The bar adds no padding of its own on top of the inset.
  it('puts the bottom inset or its own padding under the bar, not both', () => {
    expect(blocks(css, '.tab-bar')).toMatch(
      /padding:\s*var\(--space-[\d-]+\)\s+0\s+max\(var\(--space-[\d-]+\),\s*var\(--safe-bottom\)\)/,
    )
  })
})
