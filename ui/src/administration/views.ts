/** Each view of the interface, in the order the nav shows them. */
export type AdministrationView =
  | 'spend'
  | 'people'
  | 'sessions'
  | 'resources'
  | 'providers'
  | 'settings'
  | 'plugins'
  | 'health'

export const ADMINISTRATION_VIEWS: { value: AdministrationView; label: string }[] = [
  { value: 'spend', label: 'Spend' },
  { value: 'people', label: 'People' },
  { value: 'sessions', label: 'Sessions' },
  { value: 'resources', label: 'Resources' },
  { value: 'providers', label: 'Providers' },
  { value: 'settings', label: 'Settings' },
  { value: 'plugins', label: 'Plugins' },
  { value: 'health', label: 'Health' },
]

