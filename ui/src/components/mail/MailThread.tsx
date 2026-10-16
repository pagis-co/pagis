import type { ApiClient } from '../../api/client'
import { Button, Frame, Row } from '../../primitives'
import { useWorkspaceMailMessage, useWorkspaceMailThread } from '../../queries'
import { formatMoment } from '../../timeline'

export function MailThread({
  api,
  messageId,
  onBack,
}: {
  api: ApiClient
  messageId: string
  onBack: () => void
}) {
  const message = useWorkspaceMailMessage(api, messageId)
  const thread = useWorkspaceMailThread(api, message.data?.thread_id)

  return (
    <section className="mail-thread" aria-label="Mail thread">
      <Button onClick={onBack}>Back to mail</Button>
      {message.isError || thread.isError ? (
        <p role="alert">Could not load the mail thread.</p>
      ) : !thread.data ? (
        <p className="mail-empty">Loading thread…</p>
      ) : (
        <Frame>
          {thread.data.items.map((item) => (
            <Row className="mail-message" key={item.id}>
              <article>
                <h3>{item.subject || '(No subject)'}</h3>
                <p className="mail-message-sender">
                  {item.from}
                  {' · '}
                  <time dateTime={new Date(item.date).toISOString()}>
                    {formatMoment(item.date)}
                  </time>
                </p>
                <pre>{item.text}</pre>
              </article>
            </Row>
          ))}
        </Frame>
      )}
    </section>
  )
}
