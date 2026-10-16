// The replies chip: under a message with replies, the faces of
// the reply authors, the count, their names and the time of the last
// reply. A click opens the reply thread in the right panel.

import type { ApiClient } from '../api/client'
import { AvatarGroup, Button, OwnerAvatar } from '../primitives'
import { Avatar } from '../avatars/Avatar'
import { useAgents } from '../queries'
import { formatClock, type ReplyAuthor, type TimelineRow } from '../timeline'

import './RepliesChip.css'

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
  const agentOf = (author: ReplyAuthor) =>
    author.agentId === null
      ? undefined
      : agents.data?.find((agent) => agent.id === author.agentId)
  const nameOf = (author: ReplyAuthor) =>
    author.authorName

  const count = row.replyCount === 1 ? '1 reply' : `${row.replyCount} replies`
  const names = row.replyAuthors.map(nameOf).join(', ')
  const time = row.lastReplyAt === null ? null : formatClock(row.lastReplyAt)
  const label = [count, names, time].filter((part) => part !== null && part !== '').join(' · ')

  return (
    <Button variant="ghost" size="sm" className="replies-chip" onClick={onOpen}>
      {row.replyAuthors.length > 0 && (
        <AvatarGroup>
          {row.replyAuthors.map((author) =>
            author.authorKind === 'person' ? (
              <OwnerAvatar key="user" name={nameOf(author)} size="sm" />
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
