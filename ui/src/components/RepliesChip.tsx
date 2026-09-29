// The replies chip: under a message with replies, the faces of
// the reply authors, the count, their names and the time of the last
// reply. A click opens the reply thread in the right panel.

import type { ApiClient } from '../api/client'
import { Avatar, AvatarGroup, Button, OwnerAvatar } from '../primitives'
import { useAgents, useUserName } from '../queries'
import { formatClock, type ReplyAuthor, type TimelineRow } from '../timeline'

import './RepliesChip.css'

/** The name of the user in the middle of a line. */
const YOU = 'you'

export function RepliesChip({
  api,
  row,
  onOpen,
}: {
  api: ApiClient
  row: TimelineRow
  onOpen: () => void
}) {
  const agents = useAgents(api)
  const userName = useUserName(api)
  const agentOf = (author: ReplyAuthor) =>
    author.agentId === null
      ? undefined
      : agents.data?.find((agent) => agent.id === author.agentId)
  const nameOf = (author: ReplyAuthor) =>
    author.authorKind === 'user' ? YOU : (agentOf(author)?.name ?? 'Sprite')

  const count = row.replyCount === 1 ? '1 reply' : `${row.replyCount} replies`
  const names = row.replyAuthors.map(nameOf).join(', ')
  const time = row.lastReplyAt === null ? null : formatClock(row.lastReplyAt)
  const label = [count, names, time].filter((part) => part !== null && part !== '').join(' · ')

  return (
    <Button variant="ghost" size="sm" className="replies-chip" onClick={onOpen}>
      {row.replyAuthors.length > 0 && (
        <AvatarGroup>
          {row.replyAuthors.map((author) =>
            author.authorKind === 'user' ? (
              <OwnerAvatar key="user" name={userName} size="sm" />
            ) : (
              <Avatar
                key={author.agentId ?? author.authorKind}
                id={author.agentId ?? author.authorKind}
                name={nameOf(author)}
                appearance={agentOf(author)?.avatar}
                size="sm"
              />
            ),
          )}
        </AvatarGroup>
      )}
      {label}
    </Button>
  )
}
