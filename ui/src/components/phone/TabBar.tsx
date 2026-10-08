import { Link, useLocation } from '@tanstack/react-router'
import { House, MessageSquare, Users } from 'lucide-react'
import type { ApiClient } from '../../api/client'
import { avatarInitial } from '../../primitives'
import { useNeedsYou, useUserName } from '../../queries'

export function placeForPath(path: string): string {
  if (path === '/') return 'Home'
  if (path.startsWith('/c/') || path === '/conversations') return 'Conversations'
  if (path.startsWith('/sprites')) return 'Sprites'
  return 'You'
}

export function TabBar({ api }: { api: ApiClient }) {
  const path = useLocation().pathname
  const count = useNeedsYou(api).data?.count ?? 0
  const initial = avatarInitial(useUserName(api))
  const places = [
    { to: '/', label: 'Home', icon: House },
    { to: '/conversations', label: 'Conversations', icon: MessageSquare },
    { to: '/sprites', label: 'Sprites', icon: Users },
    { to: '/you', label: 'You', icon: null },
  ] as const
  return (
    <nav className="tab-bar" aria-label="Places">
      {places.map(({ to, label, icon: Icon }) => (
        <Link
          key={to}
          to={to}
          aria-label={label === 'Home' && count > 0 ? `Home, ${count} need you` : label}
          aria-current={placeForPath(path) === label ? 'page' : undefined}
          className="tab-bar-place"
        >
          <span className="tab-bar-icon" aria-hidden>
            {Icon ? <Icon size={20} /> : <span className="tab-bar-initial">{initial}</span>}
            {label === 'Home' && count > 0 && <span className="tab-bar-count">{count}</span>}
          </span>
          <span>{label}</span>
        </Link>
      ))}
    </nav>
  )
}
