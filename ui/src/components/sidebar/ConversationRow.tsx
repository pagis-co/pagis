// One conversation in the sidebar: the face with its presence
// ring, the title, the status line and the unread dot. The row reads
// the presence store, which the firehose feeds, so a ring turns
// without a refetch.
//
// The Chief of Staff's row names the designation in place of the job,
// as the roster badge does, because the designation is why the row is
// pinned first.
//
// The user's own channel with an Agent carries that Agent's presence,
// wherever it works, because it is where the user follows it. A shared
// channel carries the presence of the work done in that channel alone
// (ADR-0022), so it lights up while an Agent writes in it.

import type { AgentDto } from '../../api/client'
import { Avatar, AvatarGroup, Button, cx } from '../../primitives'
import {
  agentPresence,
  channelCaption,
  channelPresence,
  isUnread,
  usePresence,
} from '../../state/presence'
import { activityWord } from '../stateWords'
import type { ConversationRow as Row } from './conversations'

/** The faces a group row shows. A larger group says the rest in a
 *  count, because a row has no space for more. */
const STACKED_FACES = 3

function quietSince(at: number): string {
  return new Date(at).toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
  })
}

export function ConversationRow({
  row,
  agents,
  current,
  onSelect,
}: {
  row: Row
  agents: readonly AgentDto[]
  current: boolean
  onSelect: (channelId: string) => void
}) {
  // The whole store: a row is cheap to draw, and every field of it (a
  // run, a call, an unread message) changes what this row says.
  const presence = usePresence()
  const { channel, agent } = row
  const caption = channelCaption(presence, channel.id)
  const unread = isUnread(presence, channel.id)
  const live = caption !== null
  const status =
    caption ??
    (agent === null
      ? `${row.kind === 'agents' ? 'Sprites' : 'Group'} · quiet since ${quietSince(
          channel.updated_at,
        )}`
      : [
          row.kind === 'chief' ? 'Chief of Staff' : agent.job,
          activityWord(agentPresence(presence, agent.id)),
        ]
          .filter((part) => part !== '')
          .join(' · '))

  const shown = channel.agent_ids.slice(0, STACKED_FACES)
  const rest = channel.agent_ids.length - shown.length
  const agentOf = (agentId: string) => agents.find((candidate) => candidate.id === agentId)

  return (
    <Button
      variant="ghost"
      className={cx('conversation', current && 'conversation-current')}
      aria-current={current ? 'page' : undefined}
      onClick={() => onSelect(channel.id)}
    >
      {agent !== null ? (
        <Avatar
          playful
          id={agent.id}
          name={agent.name}
          appearance={agent.avatar}
          presence={agentPresence(presence, agent.id)}
        />
      ) : (
        <span className="conversation-faces">
          <AvatarGroup>
            {shown.map((agentId) => (
              <Avatar
          playful
                key={agentId}
                id={agentId}
                name={agentOf(agentId)?.name ?? '?'}
                appearance={agentOf(agentId)?.avatar}
                presence={channelPresence(presence, channel.id, agentId)}
              />
            ))}
          </AvatarGroup>
          {rest > 0 && <span className="conversation-faces-rest">+{rest}</span>}
        </span>
      )}
      <span className="conversation-text">
        <span className="conversation-title" data-testid="conversation-title">
          {channel.title ?? 'Untitled'}
        </span>
        <span
          className={cx('conversation-status', live && 'conversation-status-live')}
          data-testid="conversation-status"
        >
          {status}
        </span>
      </span>
      {unread && <span className="conversation-unread" role="status" aria-label="Unread" />}
    </Button>
  )
}
