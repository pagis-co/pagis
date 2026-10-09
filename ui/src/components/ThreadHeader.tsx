// The Thread header: who the conversation is with, what the
// Agent does now, and the acts on the whole conversation — join the
// Call, speak the replies, search the conversation and open the Desk
// panel. The face and the name of a direct message open the Agent
// itself, where the user changes it and reads its memory.

import { Link, useNavigate } from '@tanstack/react-router'
import { MoreHorizontal, Monitor, Phone, Search, Volume2, VolumeX } from 'lucide-react'
import { useState } from 'react'

import type { ApiClient } from '../api/client'
import { Avatar, IconButton, Input, Menu } from '../primitives'
import { useAgentNames, useAgents, useChannels } from '../queries'
import { agentPresence, channelCaption, liveCallOf, usePresence } from '../state/presence'
import {
  selectSpeaking,
  selectThreadQuery,
  useCallInspector,
  useSpeaking,
  useThreadSearch,
} from '../state/stores'
import { activityWord } from './stateWords'

import './ThreadHeader.css'
import { useIsMobile } from '../state/useIsMobile'
import { NavBar } from './phone/TopBar'

export type ThreadPanel = 'desk'

export function ThreadHeader({
  api,
  channelId,
  panel,
  panelIsRouteOwned = false,
  onTogglePanel,
}: {
  api: ApiClient
  channelId: string
  /** The panel open beside the Thread, if one is. */
  panel?: ThreadPanel
  /** The route decides the Desk Panel here, so there is no toggle
   *  (ADR-0022). */
  panelIsRouteOwned?: boolean
  onTogglePanel: (panel: ThreadPanel) => void
}) {
  const phone = useIsMobile()
  const navigate = useNavigate()
  const channels = useChannels(api)
  const agents = useAgents(api)
  const agentNames = useAgentNames(api)
  const channel = channels.data?.find((row) => row.id === channelId)
  // A direct message is with one Agent; the header speaks of it.
  const agentId =
    channel?.kind === 'dm' && channel.agent_ids.length === 1 ? channel.agent_ids[0] : null
  // A conversation that did not load has no title and no kind, so the
  // header names neither.
  const name =
    channel === undefined
      ? null
      : ((agentId === null ? undefined : agentNames[agentId]) ?? channel.title ?? 'Untitled')
  const appearance = agents.data?.find((agent) => agent.id === agentId)?.avatar
  const presence = usePresence((state) =>
    agentId === null ? 'none' : agentPresence(state, agentId),
  )
  const caption = usePresence((state) => channelCaption(state, channelId))
  const callId = usePresence((state) => (agentId === null ? null : liveCallOf(state, agentId)))
  const openCall = useCallInspector((state) => state.open)
  const speaking = useSpeaking(selectSpeaking(channelId))
  const toggleSpeaking = useSpeaking((state) => state.toggle)
  const query = useThreadSearch(selectThreadQuery(channelId))
  const setQuery = useThreadSearch((state) => state.set)
  const [searching, setSearching] = useState(query !== '')

  // With no Run to caption, a direct message says the Agent's
  // Activity.
  const status =
    (phone && presence !== 'working' ? null : caption) ??
    (channel === undefined
      ? null
      : agentId === null
        ? 'Group conversation'
        : activityWord(presence))

  const closeSearch = () => {
    setSearching(false)
    setQuery(channelId, '')
  }

  // A group conversation names no single Agent, so its identity is
  // words alone; a direct message carries the face and opens the Agent.
  const identity = (
    <>
      {agentId !== null && (
        <Avatar id={agentId} name={name ?? ''} appearance={appearance} playful active size={phone ? "md" : "lg"} presence={presence} />
      )}
      <div className="thread-top-title">
        {name !== null && <h1>{name}</h1>}
        {status !== null && (
          <span
            className={`thread-top-status${(presence === 'working' || (!phone && caption !== null)) ? ' thread-top-status-working' : presence === 'waiting' ? ' thread-top-status-waiting' : ''}`}
          >
            {status}
          </span>
        )}
      </div>
    </>
  )

  // The phone menu holds only what this conversation has: a search
  // where there is a message to find, and spoken replies where the
  // person posts and an Agent answers.
  const moreItems = [
    ...(channel?.last_message ? [{ label: 'Search this conversation', onSelect: () => setSearching(!searching) }] : []),
    ...(channel?.user_member && channel.agent_ids.length > 0 ? [{ label: speaking ? 'Stop speaking replies' : 'Speak replies', onSelect: () => toggleSpeaking(channelId) }] : []),
  ]

  if (phone) return <><NavBar back={{ label: 'Conversations', onBack: () => void navigate({ to: '/conversations' }) }} actions={<>
    <IconButton icon={Phone} label={name ? `Call ${name}` : 'Call'} variant="ghost" disabled={callId === null} onClick={() => { if (callId) void navigate({ to: '/calls/$callId', params: { callId } }) }} />
    {agentId && <IconButton icon={Monitor} label={`${name}'s desk`} variant="ghost" onClick={() => void navigate({ to: '/sprites/$agentId/desk', params: { agentId }, search: { from: `/c/${channelId}` } })} />}
    {moreItems.length > 0 && <Menu label="More" trigger={<IconButton icon={MoreHorizontal} label="More" variant="ghost" />} items={moreItems} />}
  </>}>{agentId ? <Link className="phone-thread-identity" to="/sprites/$agentId" params={{ agentId }}>{identity}</Link> : <div className="phone-thread-identity">{identity}</div>}</NavBar>{searching && <div className="phone-thread-search"><Input autoFocus aria-label="Search this conversation" placeholder="Search this conversation" value={query} onChange={(event) => setQuery(channelId, event.target.value)} /></div>}</>

  return (
    <header className="thread-top">
      {agentId === null ? (
        identity
      ) : (
        <Link
          to="/sprites/$agentId"
          params={{ agentId }}
          className="thread-top-identity"
          aria-label={`Open ${name}`}
          title={`Open ${name}`}
        >
          {identity}
        </Link>
      )}
      {searching && (
        <Input
          autoFocus
          className="thread-top-search"
          aria-label="Search this conversation"
          placeholder="Search this conversation"
          value={query}
          onChange={(event) => setQuery(channelId, event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Escape') closeSearch()
          }}
        />
      )}
      <div className="thread-top-actions">
        <IconButton
          icon={Phone}
          label={name === null ? 'Call' : `Call ${name}`}
          variant="ghost"
          disabled={callId === null}
          title={callId === null ? 'No call is in progress' : undefined}
          onClick={() => {
            if (callId !== null) openCall(callId)
          }}
        />
        <IconButton
          icon={speaking ? Volume2 : VolumeX}
          label="Speak replies"
          variant="ghost"
          aria-pressed={speaking}
          onClick={() => toggleSpeaking(channelId)}
        />
        <IconButton
          icon={Search}
          label="Search this conversation"
          variant="ghost"
          aria-pressed={searching}
          onClick={() => (searching ? closeSearch() : setSearching(true))}
        />
        {/* The Chief of Staff's channel owns a Desk Panel, which the
            route opens and closes (ADR-0022), so it carries no
            toggle. */}
        {!panelIsRouteOwned && (
          <IconButton
            icon={Monitor}
            label="Desk panel"
            variant="ghost"
            aria-pressed={panel === 'desk'}
            onClick={() => onTogglePanel('desk')}
          />
        )}
      </div>
    </header>
  )
}
