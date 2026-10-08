import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

import { stripComments, themes, value } from '../test/tokens'

const directory = dirname(fileURLToPath(import.meta.url))
const tokens = stripComments(readFileSync(join(directory, '../tokens.css'), 'utf8'))
const inputs = stripComments(readFileSync(join(directory, 'input.css'), 'utf8'))

// iOS zooms into a focused field when its text is smaller than 16 px.
// Check the field size for touch screens, including wide iPads.
describe('field text size', () => {
  it('uses the field token for single-line and multi-line fields', () => {
    const field = /\.ui-input\s*\{([^}]+)\}/.exec(inputs)?.[1] ?? ''
    expect(field).toMatch(/font-size:\s*var\(--text-input\)/)
  })

  it('keeps touch fields at least 16 px in both themes', () => {
    const touch = /@media\s*\(pointer:\s*coarse\)\s*\{\s*:root\s*\{([^}]+)\}/.exec(tokens)?.[1] ?? ''
    const size = /--text-input:\s*([\d.]+)rem\s*;/.exec(touch)?.[1]
    expect(size).toBeDefined()
    expect(Number(size) * 16).toBeGreaterThanOrEqual(16)
    for (const theme of themes()) {
      expect(value(theme.tokens, '--text-input')).toBe(value(theme.tokens, '--text-sm'))
    }
  })
})
