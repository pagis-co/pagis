/** Each section is a URL: `/settings/:section`. Agents are
 *  not here: the sprites have a page of their own. */
export type SettingsSection =
  | 'workspace-number'
  | 'mailbox'
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
      { value: 'workspace-number', label: 'Workspace Number' },
      { value: 'mailbox', label: 'Workspace Mailbox' },
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
    ],
  },
]

/** The sections in list order, for the palette. */
export const SETTINGS_SECTIONS: { value: SettingsSection; label: string }[] =
  SETTINGS_GROUPS.flatMap((group) => group.sections)

export function isSettingsSection(value: string): value is SettingsSection {
  return SETTINGS_SECTIONS.some((section) => section.value === value)
}

