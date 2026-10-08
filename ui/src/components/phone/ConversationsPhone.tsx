import { useState } from 'react'
import { useNavigate, useSearch } from '@tanstack/react-router'
import { Search, SquarePen } from 'lucide-react'
import type { ApiClient } from '../../api/client'
import {
  Avatar,
  AvatarGroup,
  Badge,
  Frame,
  IconButton,
  Input,
  Row,
  SectionLabel,
} from '../../primitives'
import { useAgents, useChannels, useWorkspace } from '../../queries'
import { agentPresence, channelCaption, usePresence } from '../../state/presence'
import { activityTone, activityWord } from '../stateWords'
import { conversationRows, type ConversationRow } from '../sidebar/conversations'
import { LargeTitle } from './TopBar'
import { NewGroupSheet } from './NewGroupSheet'
import { phoneTimeLabel } from '../memory/pages'

function Conversation({ api, row }: { api: ApiClient; row: ConversationRow }) {
  const agents = useAgents(api).data ?? []
  const presence = usePresence()
  const navigate = useNavigate()
  const latest = row.channel.last_message
  const activity = row.agent ? agentPresence(presence, row.agent.id) : 'none'
  const title = row.agent?.name ?? row.channel.title ?? 'Untitled'
  const preview =
    (activity === 'working' ? channelCaption(presence, row.channel.id) : null) ??
    (latest
      ? `${latest.author_kind === 'agent' && row.agent === null ? `${agents.find((agent) => agent.id === latest.author_agent_id)?.name ?? 'Sprite'}: ` : ''}${latest.text_content}`
      : row.kind === 'agents'
        ? 'You can read this conversation. You cannot post in it.'
        : 'Start with a question, a task, or an idea.')
  return (
    <Row
      className="phone-conversation-row"
      onClick={() => void navigate({ to: '/c/$channelId', params: { channelId: row.channel.id } })}
    >
      {row.agent ? (
        <Avatar
          id={row.agent.id}
          name={row.agent.name}
          appearance={row.agent.avatar}
          size="lg"
          presence={activity}
        />
      ) : (
        <AvatarGroup>
          {row.channel.agent_ids.slice(0, 2).map((id) => (
            <Avatar
              key={id}
              id={id}
              name={agents.find((agent) => agent.id === id)?.name ?? 'Sprite'}
              appearance={agents.find((agent) => agent.id === id)?.avatar}
              size="md"
            />
          ))}
        </AvatarGroup>
      )}
      <span className="phone-row-copy">
        <span className="phone-conversation-head">
          <strong>{title}</strong>
          <span className="phone-row-time">{latest ? phoneTimeLabel(latest.created_at) : ''}</span>
        </span>
        <span className="phone-conversation-preview">
          <span className="phone-row-line">{preview}</span>
          {activity !== 'idle' && activity !== 'none' && (
            <Badge tone={activityTone(activity)}>{activityWord(activity)}</Badge>
          )}
        </span>
      </span>
    </Row>
  )
}

export function ConversationsPhone({ api }: { api: ApiClient }) {
  const channels = useChannels(api)
  const agents = useAgents(api)
  const workspace = useWorkspace(api)
  const [filter, setFilter] = useState('')
  const navigate = useNavigate()
  const search = useSearch({ strict: false }) as { new?: string }
  const rows = conversationRows(
    channels.data ?? [],
    agents.data ?? [],
    workspace.data?.chief_of_staff_agent_id,
  ).filter((row) =>
    (row.agent?.name ?? row.channel.title ?? '').toLowerCase().includes(filter.toLowerCase()),
  )
  return (
    <>
      <LargeTitle
        title="Conversations"
        actions={
          <IconButton
            icon={SquarePen}
            label="New group"
            variant="link"
            onClick={() => void navigate({ to: '/conversations', search: { new: 'group' } })}
          />
        }
      />
      <div className="phone-content phone-conversations">
        <Input
          icon={Search}
          aria-label="Search conversations"
          placeholder="Search conversations"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
        {['Sprites', 'Groups'].map((group) => {
          const shown = rows.filter((row) => (row.agent !== null) === (group === 'Sprites'))
          return shown.length ? (
            <section className="phone-section" key={group}>
              <SectionLabel>{group}</SectionLabel>
              <Frame>
                {shown.map((row) => (
                  <Conversation api={api} row={row} key={row.channel.id} />
                ))}
              </Frame>
            </section>
          ) : null
        })}
        {(channels.isError || agents.isError) && (
          <p role="alert" className="phone-hint">
            Could not read the conversations.
          </p>
        )}
        {!channels.isError && !agents.isError && !rows.length && (
          <p className="phone-hint">
            {channels.isPending
              ? 'Loading conversations…'
              : filter.trim()
                ? 'No conversations match the search.'
                : 'No conversations yet. Start with a question, a task, or an idea.'}
          </p>
        )}
      </div>
      <NewGroupSheet
        api={api}
        open={search.new === 'group'}
        onClose={() => void navigate({ to: '/conversations', search: {} })}
        onCreated={(channelId) => void navigate({ to: '/c/$channelId', params: { channelId } })}
      />
    </>
  )
}
