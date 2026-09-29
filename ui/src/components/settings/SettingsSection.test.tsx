// The settings grammar: a title, a one-sentence lead, at
// most one primary action at the right, the framed lists, and a closing
// hint that says what the page cannot do.

import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { SettingsSection } from './SettingsSection'

describe('SettingsSection', () => {
  it('writes the title, the lead and the action on one line', () => {
    render(
      <SettingsSection
        title="Vault"
        lead="Sign-ins your sprites can fill."
        action={<button>Add a sign-in</button>}
        hint="The password stays in the keychain."
      >
        <p>rows</p>
      </SettingsSection>,
    )

    const heading = screen.getByRole('heading', { level: 3, name: 'Vault' })
    const head = heading.parentElement as HTMLElement
    expect(head.className).toContain('settings-section-head')
    expect(head.contains(screen.getByText('Sign-ins your sprites can fill.'))).toBe(true)
    expect(head.contains(screen.getByRole('button', { name: 'Add a sign-in' }))).toBe(true)
    expect(screen.getByText('rows')).toBeTruthy()
    const hint = screen.getByText('The password stays in the keychain.')
    expect(hint.className).toContain('settings-section-hint')
  })

  it('renders without an action and without a hint', () => {
    const { container } = render(
      <SettingsSection title="Sound" lead="A chime when an Agent needs you.">
        <p>rows</p>
      </SettingsSection>,
    )
    expect(container.querySelector('.settings-section-hint')).toBeNull()
    expect(screen.queryByRole('button')).toBeNull()
  })
})
