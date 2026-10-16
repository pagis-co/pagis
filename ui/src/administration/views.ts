/** Each view of the interface, in the order the nav shows them. */
export type AdministrationView =
  | 'spend'
  | 'people'
  | 'workspaces'
  | 'person-defaults'
  | 'sessions'
  | 'resources'
  | 'providers'
  | 'settings'
  | 'plugins'
  | 'health'
  | 'junk'

export const ADMINISTRATION_VIEWS: { value: AdministrationView; label: string }[] = [
  { value: 'spend', label: 'Spend' },
  { value: 'people', label: 'People' },
  { value: 'workspaces', label: 'Workspaces' },
  { value: 'person-defaults', label: 'New People' },
  { value: 'sessions', label: 'Sessions' },
  { value: 'resources', label: 'Resources' },
  { value: 'providers', label: 'Providers' },
  { value: 'settings', label: 'Settings' },
  { value: 'plugins', label: 'Plugins' },
  { value: 'health', label: 'Health' },
  { value: 'junk', label: 'Junk' },
]

