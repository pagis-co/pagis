// The settings nav: the three groups on the 220 px shell, the
// open section marked as the page, and a click that opens another.

import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { SettingsNav } from './SettingsNav'

describe('SettingsNav', () => {
  it('lists the sections in three groups under the Settings title', () => {
    render(<SettingsNav section="vault" onSelectSection={() => {}} isAdministrator />)

    const nav = screen.getByRole('navigation', { name: 'Settings' })
    expect(nav.className).toContain('ui-settings-nav')
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeTruthy()
    for (const group of ['Access', 'Models', 'System']) {
      expect(screen.getByRole('group', { name: group })).toBeTruthy()
    }
    expect(
      screen.getByRole('button', { name: 'Vault' }).getAttribute('aria-current'),
    ).toBe('page')
    expect(
      screen.getByRole('button', { name: 'Connections' }).getAttribute('aria-current'),
    ).toBeNull()
  })

  it('lists Notifications in the System group, after Sound', () => {
    render(<SettingsNav section="vault" onSelectSection={() => {}} isAdministrator />)

    const system = within(screen.getByRole('group', { name: 'System' }))
    expect(system.getAllByRole('button').map((button) => button.textContent)).toEqual([
      'Retention',
      'Timezone',
      'Sound',
      'Notifications',
      'Administration',
    ])
  })

  it('opens the section the user clicks', async () => {
    const onSelectSection = vi.fn()
    render(
      <SettingsNav section="vault" onSelectSection={onSelectSection} isAdministrator />,
    )

    await userEvent.click(screen.getByRole('button', { name: 'Sound' }))
    expect(onSelectSection).toHaveBeenCalledWith('sound')
  })

  // The installation's settings answer on the administration port, so
  // the product holds none of them. An administrator gets one link to
  // it, and a member not even that.
  it('leaves the Administration section out for a member', () => {
    render(
      <SettingsNav
        section="vault"
        onSelectSection={() => {}}
        isAdministrator={false}
      />,
    )

    expect(screen.queryByRole('button', { name: 'Administration' })).toBeNull()
    // The group keeps the sections a member may open.
    expect(screen.getByRole('group', { name: 'System' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Retention' })).toBeTruthy()
    // A person always reads what they spent, whatever their role.
    expect(screen.getByRole('button', { name: 'Usage' })).toBeTruthy()
  })

  it('gives an administrator the Administration link', () => {
    render(
      <SettingsNav section="administration" onSelectSection={() => {}} isAdministrator />,
    )

    expect(
      screen.getByRole('button', { name: 'Administration' }).getAttribute('aria-current'),
    ).toBe('page')
  })
})
