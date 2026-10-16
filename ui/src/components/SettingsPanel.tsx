import { Members } from './settings/Members'
import { WorkspaceNumber } from './settings/WorkspaceNumber'
import { useState, type ReactNode } from 'react'
import { useLocation, useNavigate } from '@tanstack/react-router'
import { useIsMobile } from '../primitives/useMediaQuery'
import { HeaderActions, NavBar } from './phone/TopBar'
import { ConnectionsPhone } from './phone/ConnectionsPhone'

import type { ApiClient } from '../api/client'
import { useUser } from '../queries'
import type { AdministrationAddress } from './settings/OrgSettingsGroup'
import { WorkspaceMailbox } from './settings/WorkspaceMailbox'
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

import type { SettingsSection } from './settings/groups'

import './SettingsPanel.css'
import './settings.css'

/** The frame every settings address draws in: the section nav and the
 *  content of the open section. A connection page is a content
 *  of its own, so the frame takes children. */
export function SettingsShell({
  section,
  onSelectSection,
  isAdministrator,
  administration,
  children,
}: {
  section: SettingsSection
  onSelectSection: (section: SettingsSection) => void
  /** Whether the Person sees the Org group. */
  isAdministrator: boolean
  administration?: AdministrationAddress | null
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
        administration={administration}
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
  /** Whether the Person sees the Org group. */
  isAdministrator: boolean
}) {
  const phone = useIsMobile()
  const administration = useUser(api).data?.administration
  return (
    <SettingsShell
      section={section}
      onSelectSection={onSelectSection}
      isAdministrator={isAdministrator}
      administration={administration}
    >
      {section === 'members' && <Members api={api} />}
      {section === 'mailbox' && <WorkspaceMailbox api={api} />}
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
      {section === 'workspace-number' && <WorkspaceNumber api={api} />}
      {section === 'retention' && <Retention api={api} />}
      {section === 'timezone' && <TimezoneSection api={api} />}
      {section === 'sound' && <SoundSection />}
      {section === 'notifications' && <Notifications api={api} />}
    </SettingsShell>
  )
}
