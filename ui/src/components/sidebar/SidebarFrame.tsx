import { Ellipsis, LogOut, Search } from 'lucide-react'
import type { ReactNode } from 'react'
import type { components } from '../../api/schema'
import { Button, ButtonLink, cx, IconButton, LogoMark, Menu, OwnerAvatar, Sidebar } from '../../primitives'
import { OrgHead } from '../OrgHead'
import { PersonNotice } from '../PersonNotice'
import { PLACES, type PlaceId } from './places'
import './Sidebar.css'

export interface SidebarFrameProps {
  userName: string
  org?: components['schemas']['OrgDto']
  role: string
  links?: { label: string; url: string }[]
  captureDays?: number | null
  notice?: components['schemas']['PersonNoticeDto'] | null
  currentPlace: PlaceId | null
  needsYou?: number
  publicOrigin?: string
  onSelectPlace?: (place: PlaceId) => void
  onSearch?: () => void
  onSignOut: () => void
  signOutPending?: boolean
  children?: ReactNode
}

/** The shared sidebar takes Person data and navigation targets. */
export function SidebarFrame({
  userName,
  org,
  role,
  links = [],
  captureDays = null,
  notice,
  currentPlace,
  needsYou = 0,
  publicOrigin,
  onSelectPlace,
  onSearch,
  onSignOut,
  signOutPending = false,
  children,
}: SidebarFrameProps) {
  const profile = (
    <>
      <OwnerAvatar name={userName} size="md" />
      <span>{userName}</span>
    </>
  )
  return (
    <Sidebar className="sidebar">
      <div className="sidebar-header">
        <span className="sidebar-brand">
          <LogoMark />
          Pagis
        </span>
        {onSearch && <IconButton icon={Search} label="Search" variant="ghost" size="sm" onClick={onSearch} />}
      </div>
      {org && <OrgHead org={org} role={role} />}
      <nav className="sidebar-places" aria-label="Places">
        {PLACES.map((place) => {
          const content = (
            <>
              <place.icon className="place-icon" aria-hidden />
              <span>{place.label}</span>
              {place.id === 'home' && needsYou > 0 && <span className="place-count" aria-label={`${needsYou} need you`}>{needsYou}</span>}
            </>
          )
          const props = {
            variant: 'ghost' as const,
            className: cx('place', place.id === currentPlace && 'place-current'),
            'aria-current': place.id === currentPlace ? 'page' as const : undefined,
          }
          return publicOrigin ? (
            <ButtonLink key={place.id} {...props} href={`${publicOrigin}${place.id === 'home' ? '/' : `/${place.id}`}`}>{content}</ButtonLink>
          ) : (
            <Button key={place.id} {...props} onClick={() => onSelectPlace?.(place.id)}>{content}</Button>
          )
        })}
      </nav>
      {children}
      {captureDays !== null && <p className="sidebar-capture-notice" role="note">Pagis keeps a copy of your model requests for {captureDays} days.</p>}
      <PersonNotice notice={notice} className="sidebar-notice" />
      <div className="sidebar-profile">
        {publicOrigin ? (
          <ButtonLink variant="ghost" className="sidebar-profile-open" href={`${publicOrigin}/settings`}>{profile}</ButtonLink>
        ) : (
          <Button variant="ghost" className="sidebar-profile-open" onClick={() => onSelectPlace?.('settings')}>{profile}</Button>
        )}
        {links.length > 0 && (
          <Menu
            label="Account menu"
            trigger={<IconButton icon={Ellipsis} label="Account menu" variant="ghost" size="sm" />}
            items={links.map((link) => ({ label: link.label, href: link.url }))}
          />
        )}
        <IconButton icon={LogOut} label="Sign out" variant="ghost" size="sm" disabled={signOutPending} onClick={onSignOut} />
      </div>
    </Sidebar>
  )
}
