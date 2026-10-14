
import { Button, ButtonLink, SectionLabel, SettingsNav as Shell, cx } from '../../primitives'
import type { AdministrationView } from '../../administration/views'
import { type SettingsSection, SETTINGS_GROUPS } from '../SettingsPanel'

import { OrgSettingsGroup, type AdministrationAddress } from './OrgSettingsGroup'

import './SettingsNav.css'

/** The 220 px settings nav: the Settings title, then the
 * sections and the Org links in their groups. The open section is the page. */
export function SettingsNav({
  section,
  onSelectSection,
  isAdministrator,
  administration,
  publicOrigin,
  currentView,
  onSelectView,
}: {
  section?: SettingsSection
  onSelectSection?: (section: SettingsSection) => void
  /** A Member sees no Org group. */
  isAdministrator: boolean
  administration?: AdministrationAddress | null
  publicOrigin?: string
  currentView?: AdministrationView
  onSelectView?: (view: AdministrationView) => void
}) {

  return (
    <Shell>
      <div className="settings-nav-header">
        <h2 className="settings-nav-title">Settings</h2>
      </div>
      {SETTINGS_GROUPS.map((group) => (
        <div
          key={group.label}
          className="settings-nav-group"
          role="group"
          aria-label={group.label}
        >
          <SectionLabel>{group.label}</SectionLabel>
          {group.sections.map((item) => (
            publicOrigin ? <ButtonLink key={item.value} variant="ghost" shape="nav" className="settings-nav-item"
              href={`${publicOrigin}/settings/${item.value}`}>{item.label}</ButtonLink> : <Button
              key={item.value}
              variant="ghost"
              shape="nav"
              className={cx(
                'settings-nav-item',
                item.value === section && 'settings-nav-item-active',
              )}
              aria-current={item.value === section ? 'page' : undefined}
              onClick={() => onSelectSection?.(item.value)}
            >
              {item.label}
            </Button>
          ))}
        </div>
      ))}
      <OrgSettingsGroup administration={administration} isAdministrator={isAdministrator} currentView={currentView} onSelectView={onSelectView} />
    </Shell>
  )
}
