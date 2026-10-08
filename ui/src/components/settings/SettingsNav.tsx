import { Menu as MenuIcon } from 'lucide-react'

import { Button, IconButton, SectionLabel, SettingsNav as Shell, cx } from '../../primitives'
import { useMobileNav } from '../../state/stores'
import { type SettingsSection, visibleSettingsGroups } from '../SettingsPanel'

import './SettingsNav.css'

/** The 220 px settings nav: the Settings title, then the
 * sections in their three groups. The open section is the page. */
export function SettingsNav({
  section,
  onSelectSection,
  isAdministrator,
}: {
  section: SettingsSection
  onSelectSection: (section: SettingsSection) => void
  /** A member sees no administrator section. */
  isAdministrator: boolean
}) {
  const openNav = useMobileNav((state) => state.open)
  const navOpen = useMobileNav((state) => state.isOpen)

  return (
    <Shell>
      <div className="settings-nav-header">
        <IconButton
          icon={MenuIcon}
          label="Open conversations"
          variant="ghost"
          className="mobile-navigation-trigger"
          aria-expanded={navOpen}
          onClick={openNav}
        />
        <h2 className="settings-nav-title">Settings</h2>
      </div>
      {visibleSettingsGroups(isAdministrator).map((group) => (
        <div
          key={group.label}
          className="settings-nav-group"
          role="group"
          aria-label={group.label}
        >
          <SectionLabel>{group.label}</SectionLabel>
          {group.sections.map((item) => (
            <Button
              key={item.value}
              variant="ghost"
              className={cx(
                'settings-nav-item',
                item.value === section && 'settings-nav-item-active',
              )}
              aria-current={item.value === section ? 'page' : undefined}
              onClick={() => onSelectSection(item.value)}
            >
              {item.label}
            </Button>
          ))}
        </div>
      ))}
    </Shell>
  )
}
