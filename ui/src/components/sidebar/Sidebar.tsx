// The sidebar (ADR-0022): the six places at the top with the
// Needs-You count on Home, the conversations under them with the Chief
// of Staff first, and the profile row at the bottom. The profile row
// shows the name the wizard recorded and opens Settings. The row also
// ends the session.
// One current mark at a time: a
// conversation row in a thread, a place elsewhere. On a phone the same
// column is replaced by the phone layout at 760 px and below.

import { LogOut, Plus, Search } from 'lucide-react'
import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import {
  Button,
  cx,
  IconButton,
  LogoMark,
  OwnerAvatar,
  Sidebar as SidebarShell,
} from '../../primitives'
import {
  useAgents,
  useChannels,
  useNeedsYou,
  useUser,
  useUserName,
  useWorkspace,
} from '../../queries'
import { ConversationRow } from './ConversationRow'
import { conversationRows } from './conversations'
import { NewGroupForm } from './NewGroupForm'
import { PLACES, placeForPath, type PlaceId } from './places'

import './Sidebar.css'

export function Sidebar({
  api,
  pathname,
  selectedId,
  onSelectPlace,
  onSelectChannel,
  onSearch,
  onSignOut,
}: {
  api: ApiClient
  pathname: string
  /** The open conversation, which holds the current mark in a thread. */
  selectedId: string | null
  onSelectPlace: (place: PlaceId) => void
  onSelectChannel: (channelId: string) => void
  onSearch: () => void
  /** End the session and go back to the sign-in page. */
  onSignOut: () => void
}) {
  const channels = useChannels(api)
  const roster = useAgents(api)
  const workspace = useWorkspace(api)
  const userName = useUserName(api)
  const captureDays = useUser(api).data?.model_request_capture_days ?? null
  const needsYou = useNeedsYou(api).data?.count ?? 0
  const [creating, setCreating] = useState(false)
  // A conversation holds the mark, so no place does.
  const currentPlace = selectedId === null ? placeForPath(pathname) : null
  const rows = conversationRows(
    channels.data ?? [],
    roster.data ?? [],
    workspace.data?.chief_of_staff_agent_id,
  )

  return (
    <SidebarShell className="sidebar">
      <div className="sidebar-header">
        <span className="sidebar-brand">
          <LogoMark />
          Pagis
        </span>
        <IconButton icon={Search} label="Search" variant="ghost" size="sm" onClick={onSearch} />
      </div>

      <nav className="sidebar-places" aria-label="Places">
        {PLACES.map((place) => (
          <Button
            key={place.id}
            variant="ghost"
            className={cx('place', place.id === currentPlace && 'place-current')}
            aria-current={place.id === currentPlace ? 'page' : undefined}
            onClick={() => onSelectPlace(place.id)}
          >
            <place.icon size={16} aria-hidden />
            <span>{place.label}</span>
            {place.id === 'home' && needsYou > 0 && (
              <span className="place-count" aria-label={`${needsYou} need you`}>
                {needsYou}
              </span>
            )}
          </Button>
        ))}
      </nav>

      <nav className="sidebar-conversations" aria-label="Conversations">
        <div className="sidebar-conversations-heading">
          <span>Conversations</span>
          <IconButton
            icon={Plus}
            label="New group"
            variant="ghost"
            size="sm"
            onClick={() => setCreating(true)}
          />
        </div>
        {creating && (
          <NewGroupForm
            api={api}
            onCreated={(channelId) => {
              setCreating(false)
              onSelectChannel(channelId)
            }}
            onCancel={() => setCreating(false)}
          />
        )}
        {rows.map((row) => (
          <ConversationRow
            key={row.channel.id}
            row={row}
            agents={roster.data ?? []}
            current={row.channel.id === selectedId}
            onSelect={onSelectChannel}
          />
        ))}
        {channels.data?.length === 0 && (
          <p className="sidebar-empty">No conversations yet.</p>
        )}
      </nav>

      {captureDays !== null && (
        <p className="sidebar-capture-notice" role="note">
          Pagis keeps a copy of your model requests for {captureDays} days.
        </p>
      )}

      <div className="sidebar-profile">
        <Button
          variant="ghost"
          className="sidebar-profile-open"
          onClick={() => onSelectPlace('settings')}
        >
          <OwnerAvatar name={userName} size="md" />
          <span>{userName}</span>
        </Button>
        <IconButton
          icon={LogOut}
          label="Sign out"
          variant="ghost"
          size="sm"
          onClick={onSignOut}
        />
      </div>
    </SidebarShell>
  )
}
