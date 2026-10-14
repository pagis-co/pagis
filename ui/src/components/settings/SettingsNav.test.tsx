// The settings nav: the three groups on the 220 px shell, the
// open section marked as the page, and a click that opens another.

import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { SettingsNav } from './SettingsNav'

const address = { origin: 'https://pagis.example.net', path_prefix: '/admin', loopback: false, sections: ['people', 'spend', 'hosts', 'restart'], links: [{ label: 'Help', url: 'https://team.example.com/help' }] }

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

  it('shows offered views and configured links in Org after System', () => {
    render(<SettingsNav section="vault" onSelectSection={() => {}} isAdministrator administration={address} />)
    const groups = screen.getAllByRole('group')
    expect(groups.map((group) => group.getAttribute('aria-label'))).toEqual(['Access', 'Models', 'System', 'Org'])
    const org = within(screen.getByRole('group', { name: 'Org' }))
    expect(org.getAllByRole('link').map((link) => [link.textContent, link.getAttribute('href')])).toEqual([
      ['People', 'https://pagis.example.net/admin/people'],
      ['Spend', 'https://pagis.example.net/admin/spend'],
      ['Help', 'https://team.example.com/help'],
    ])
    expect(org.queryByRole('link', { name: 'Providers' })).toBeNull()
    expect(org.queryByRole('button')).toBeNull()
    expect(org.getAllByRole('link').every((link) => !link.hasAttribute('target'))).toBe(true)
    expect(org.getAllByRole('link').every((link) => !link.hasAttribute('aria-current'))).toBe(true)
  })

  it('shows no Org group to a Member, even with an address', () => {
    render(<SettingsNav section="vault" onSelectSection={() => {}} isAdministrator={false} administration={address} />)
    expect(screen.queryByRole('group', { name: 'Org' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Usage' })).toBeTruthy()
  })

  it('shows no Org group before the address arrives', () => {
    render(<SettingsNav section="vault" onSelectSection={() => {}} isAdministrator />)
    expect(screen.queryByRole('group', { name: 'Org' })).toBeNull()
  })

  it.each([
    ['http://127.0.0.1:4401', '4401'],
    ['http://localhost', '80'],
    ['https://localhost', '443'],
  ])('shows the SSH tunnel below the items for %s', (origin, port) => {
    render(<SettingsNav section="vault" onSelectSection={() => {}} isAdministrator administration={{ ...address, origin, loopback: true }} />)
    const org = screen.getByRole('group', { name: 'Org' })
    const command = within(org).getByText(`ssh -L ${port}:127.0.0.1:${port} you@your-server`)
    const links = within(org).getAllByRole('link')
    expect(links.at(-1)!.compareDocumentPosition(command) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('shows no tunnel for a proxy address', () => {
    render(<SettingsNav section="vault" onSelectSection={() => {}} isAdministrator administration={address} />)
    expect(screen.queryByText(/ssh -L/)).toBeNull()
  })
})
