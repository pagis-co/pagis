import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import { ButtonLink } from './button-link'

it('draws a native link with the button styles and opens in the same tab', () => {
  render(<ButtonLink href="https://pagis.example.net/admin/people" variant="ghost" shape="nav" className="settings-nav-item">People</ButtonLink>)
  const link = screen.getByRole('link', { name: 'People' })
  expect(link.getAttribute('href')).toBe('https://pagis.example.net/admin/people')
  expect(link.hasAttribute('target')).toBe(false)
  expect(link.className).toContain('ui-button-ghost')
  expect(link.className).toContain('settings-nav-item')
  expect(link.className).toContain('ui-button-nav')
})
