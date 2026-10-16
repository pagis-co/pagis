// The places of the sidebar. Each one replaces the main
// pane. A conversation is not a place: the list under the places is
// the door to it.

import {
  BookOpen,
  CalendarDays,
  Contact,
  Mail,
  Home,
  Package,
  RefreshCw,
  Settings,
  SquareTerminal,
  Users,
  type LucideIcon,
} from 'lucide-react'

export type PlaceId =
  | 'home'
  | 'people'
  | 'sprites'
  | 'mail'
  | 'calendar'
  | 'memory'
  | 'automations'
  | 'coding'
  | 'software'
  | 'settings'

export interface Place {
  id: PlaceId
  label: string
  icon: LucideIcon
}

export const PLACES: readonly Place[] = [
  { id: 'home', label: 'Home', icon: Home },
  { id: 'people', label: 'People', icon: Contact },
  { id: 'sprites', label: 'Sprites', icon: Users },
  { id: 'mail', label: 'Mail', icon: Mail },
  { id: 'calendar', label: 'Calendar', icon: CalendarDays },
  { id: 'memory', label: 'Memory', icon: BookOpen },
  { id: 'automations', label: 'Automations', icon: RefreshCw },
  { id: 'coding', label: 'Coding', icon: SquareTerminal },
  { id: 'software', label: 'Software', icon: Package },
  { id: 'settings', label: 'Settings', icon: Settings },
]

/** The place a path belongs to, or `null` for a path off the places (a
 * conversation, a run). The path decides, so a reload keeps the mark. */
export function placeForPath(pathname: string): PlaceId | null {
  const [first] = pathname.split('/').filter((part) => part !== '')
  if (first === undefined) return 'home'
  // An Agent is one of the user's sprites.
  if (first === 'sprites' || first === 'agents') return 'sprites'
  if (first === 'calendar') return 'calendar'
  if (first === 'mail') return 'mail'
  if (first === 'people' || first === 'conversations') return 'people'
  if (first === 'memory') return 'memory'
  if (first === 'automations') return 'automations'
  if (first === 'coding') return 'coding'
  if (first === 'software') return 'software'
  if (first === 'settings') return 'settings'
  return null
}
