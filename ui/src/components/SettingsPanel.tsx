import { useState, type ReactNode } from 'react'
import { useLocation, useNavigate } from '@tanstack/react-router'
import { useIsMobile } from '../state/useIsMobile'
import { HeaderActions, NavBar } from './phone/TopBar'
import { ConnectionsPhone } from './phone/ConnectionsPhone'

import type { ApiClient } from '../api/client'
import { AdministrationLink } from './settings/AdministrationLink'
import { Connections } from './Connections'
import { HomeExit } from './settings/HomeExit'
import { HOST_HINT, Hosts } from './settings/Hosts'
import { ModelsSettings } from './ModelsSettings'
import { Notifications } from './settings/Notifications'
import { Retention } from './settings/Retention'
import { Sessions } from './settings/Sessions'
import { Usage } from './settings/Usage'
import { SettingsNav } from './settings/SettingsNav'
import { Vault } from './settings/Vault'
import { SoundSection } from './SoundSection'
import { TimezoneSection } from './settings/Timezone'
import { TrustedContacts } from './TrustedContacts'

import './SettingsPanel.css'
import './settings.css'

/** Each section is a URL: `/settings/:section`. Agents are
 *  not here: the sprites have a page of their own. */
export type SettingsSection =
  | 'connections'
  | 'hosts'
  | 'sessions'
  | 'vault'
  | 'trusted-contacts'
  | 'models'
  | 'usage'
  | 'retention'
  | 'timezone'
  | 'sound'
  | 'notifications'
  | 'administration'

export interface SettingsGroup {
  label: string
  sections: { value: SettingsSection; label: string }[]
}

/** Three groups: who reaches the Workspace and whom the agents reach,
 *  what they think with, and the machine they run on. */
export const SETTINGS_GROUPS: SettingsGroup[] = [
  {
    label: 'Access',
    sections: [
      { value: 'connections', label: 'Connections' },
      { value: 'hosts', label: 'Hosts' },
      { value: 'sessions', label: 'Sessions' },
      { value: 'vault', label: 'Vault' },
      { value: 'trusted-contacts', label: 'Trusted contacts' },
    ],
  },
  {
    label: 'Models',
    sections: [
      { value: 'models', label: 'Models' },
      { value: 'usage', label: 'Usage' },
    ],
  },
  {
    label: 'System',
    sections: [
      { value: 'retention', label: 'Retention' },
      { value: 'timezone', label: 'Timezone' },
      { value: 'sound', label: 'Sound' },
      { value: 'notifications', label: 'Notifications' },
      { value: 'administration', label: 'Administration' },
    ],
  },
]

/** The sections in list order, for the palette. */
export const SETTINGS_SECTIONS: { value: SettingsSection; label: string }[] =
  SETTINGS_GROUPS.flatMap((group) => group.sections)

/** The sections only an administrator sees. The installation's own
 *  settings, people and Plugins answer on the administration port, so
 *  the product draws none of them: an administrator gets one link to
 *  the Administration Interface. A member's own Usage is not here: a
 *  person may always read what they spent. */
export const ADMINISTRATOR_SECTIONS: SettingsSection[] = ['administration']

export function isAdministratorSection(section: SettingsSection): boolean {
  return ADMINISTRATOR_SECTIONS.includes(section)
}

/** The groups one person sees: every group for an administrator, and
 *  the groups without the administrator sections for a member. A group
 *  that keeps no section is left out. */
export function visibleSettingsGroups(isAdministrator: boolean): SettingsGroup[] {
  if (isAdministrator) return SETTINGS_GROUPS
  return SETTINGS_GROUPS.map((group) => ({
    ...group,
    sections: group.sections.filter((section) => !isAdministratorSection(section.value)),
  })).filter((group) => group.sections.length > 0)
}

/** The sections one person may open, in list order. */
export function visibleSettingsSections(
  isAdministrator: boolean,
): { value: SettingsSection; label: string }[] {
  return visibleSettingsGroups(isAdministrator).flatMap((group) => group.sections)
}

export function isSettingsSection(value: string): value is SettingsSection {
  return SETTINGS_SECTIONS.some((section) => section.value === value)
}

/** The frame every settings address draws in: the section nav and the
 *  content of the open section. A connection page is a content
 *  of its own, so the frame takes children. */
export function SettingsShell({
  section,
  onSelectSection,
  isAdministrator,
  children,
}: {
  section: SettingsSection
  onSelectSection: (section: SettingsSection) => void
  /** Whether the person may see the administrator sections. */
  isAdministrator: boolean
  children: ReactNode
}) {
  const phone = useIsMobile()
  if (phone) return <PhoneSettingsShell section={section}>{children}</PhoneSettingsShell>
  return (
    <div className="settings-panel">
      <SettingsNav
        section={section}
        onSelectSection={onSelectSection}
        isAdministrator={isAdministrator}
      />
      <div className="settings-content">{children}</div>
    </div>
  )
}

function PhoneSettingsShell({ section, children }: { section: SettingsSection; children: ReactNode }) {
  const navigate = useNavigate()
  const location = useLocation()
  const [actions, setActions] = useState<HTMLDivElement | null>(null)
  const connectionDetail = section === 'connections' && location.pathname !== '/settings/connections'
  return <HeaderActions.Provider value={actions}><NavBar back={{ label: connectionDetail ? 'Connections' : 'Settings', onBack: () => void navigate({ href: connectionDetail ? '/settings/connections' : '/settings' }) }} actions={<div className="phone-header-actions" ref={setActions} />} /><div className="phone-content settings-phone">{children}</div></HeaderActions.Provider>
}

export function SettingsPanel({
  api,
  section,
  onSelectSection,
  onOpenConnection,
  isAdministrator,
}: {
  api: ApiClient
  section: SettingsSection
  onSelectSection: (section: SettingsSection) => void
  /** The connection page one card opens. */
  onOpenConnection: (connectionId: string) => void
  /** Whether the person may see the administrator sections. */
  isAdministrator: boolean
}) {
  const phone = useIsMobile()
  return (
    <SettingsShell
      section={section}
      onSelectSection={onSelectSection}
      isAdministrator={isAdministrator}
    >
      {section === 'connections' && (
        phone ? <ConnectionsPhone api={api} onOpen={onOpenConnection} /> : <section className="settings-section">
          <h3>Connections</h3>
          <Connections api={api} onOpen={onOpenConnection} />
        </section>
      )}
      {section === 'hosts' && (
        <>
          <Hosts api={api} />
          <HomeExit api={api} />
          {phone && <p className="phone-hint">{HOST_HINT}</p>}
        </>
      )}
      {section === 'sessions' && <Sessions api={api} />}
      {section === 'vault' && <Vault api={api} />}
      {section === 'trusted-contacts' && <TrustedContacts api={api} />}
      {section === 'models' && <ModelsSettings api={api} />}
      {section === 'usage' && <Usage api={api} />}
      {section === 'retention' && <Retention api={api} />}
      {section === 'timezone' && <TimezoneSection api={api} />}
      {section === 'sound' && <SoundSection />}
      {section === 'notifications' && <Notifications api={api} />}
      {section === 'administration' && isAdministrator && <AdministrationLink api={api} />}
    </SettingsShell>
  )
}
