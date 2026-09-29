// The SectionLabel names a group of rows or nav items.

import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { SectionLabel } from './section-label'

describe('SectionLabel', () => {
  it('renders the words in a labelled element', () => {
    render(<SectionLabel>Access</SectionLabel>)
    expect(screen.getByText('Access').className).toContain('ui-section-label')
  })

  it('carries the screen class next to its own', () => {
    render(<SectionLabel className="settings-group">Models</SectionLabel>)
    expect(screen.getByText('Models').className).toContain('settings-group')
  })
})
