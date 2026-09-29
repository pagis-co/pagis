// The four layout shells: the sidebar,
// the reading column, the settings nav and the panel. Each one is a
// landmark with a fixed width from the tokens and no page code.

import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { Panel, ReadingColumn, SettingsNav, Sidebar } from './layout'

describe('the layout shells', () => {
  it('renders the sidebar as a complementary landmark', () => {
    render(<Sidebar>nav</Sidebar>)
    const sidebar = screen.getByRole('complementary', { name: 'Sidebar' })
    expect(sidebar.className).toContain('ui-sidebar')
    expect(sidebar.textContent).toBe('nav')
  })

  it('renders the reading column with the screen class', () => {
    render(<ReadingColumn className="thread">words</ReadingColumn>)
    const column = screen.getByText('words')
    expect(column.className).toContain('ui-reading-column')
    expect(column.className).toContain('thread')
  })

  it('renders the settings nav as a navigation landmark', () => {
    render(<SettingsNav>links</SettingsNav>)
    const nav = screen.getByRole('navigation', { name: 'Settings' })
    expect(nav.className).toContain('ui-settings-nav')
  })

  it('renders the panel as a complementary landmark with the given name', () => {
    render(<Panel label="Replies">thread</Panel>)
    const panel = screen.getByRole('complementary', { name: 'Replies' })
    expect(panel.className).toContain('ui-panel')
  })
})
