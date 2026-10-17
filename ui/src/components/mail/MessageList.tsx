import { Button, Frame } from '../../primitives'
import { MessageRow, type MailSummary } from './MessageRow'

export function MessageList({
  messages,
  emptyText = 'No mail found.',
  loading,
  failed,
  selectedId,
  onOpen,
  cursor,
  next,
  onPage,
}: {
  emptyText?: string
  messages: MailSummary[] | undefined
  loading: boolean
  failed: boolean
  selectedId: string | null
  onOpen: (id: string) => void
  cursor: string | undefined
  next: string | null | undefined
  onPage: (cursor: string | undefined) => void
}) {
  return (
    <section className="mail-list" aria-label="Messages">
      {failed ? (
        <p role="alert">Could not load mail.</p>
      ) : loading ? (
        <p className="mail-empty">Loading mail…</p>
      ) : messages?.length === 0 ? (
        <p className="mail-empty">{emptyText}</p>
      ) : (
        <Frame>
          {messages?.map((message) => (
            <MessageRow
              key={message.id}
              message={message}
              selected={selectedId === message.id}
              onOpen={() => onOpen(message.id)}
            />
          ))}
        </Frame>
      )}
      <div className="mail-pages">
        {cursor && (
          <Button onClick={() => onPage(undefined)}>Newest mail</Button>
        )}
        {next && <Button onClick={() => onPage(next)}>Older mail</Button>}
      </div>
    </section>
  )
}
