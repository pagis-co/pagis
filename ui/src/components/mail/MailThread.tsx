import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Button, Frame, Row } from '../../primitives'
import {
  type MailSource,
  useWorkspaceMailMessage,
  useWorkspaceMailThread,
} from '../../queries'
import { Composer } from './Composer'
import { MessageControls } from './MessageControls'
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
  const [source, setSource] = useState<MailSource>()
  const message = useWorkspaceMailMessage(api, messageId)
  const thread = useWorkspaceMailThread(api, message.data?.thread_id)

  return (
    <section className="mail-thread" aria-label="Mail thread">
      <Button onClick={onBack}>Back to mail</Button>
      {source && (
        <Composer
          key={`${source.id}:${source.action}`}
          api={api}
          source={source}
          onClose={() => setSource(undefined)}
        />
      )}
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
                <ul>
                  {item.attachments?.map((file, index) => (
                    <li key={index}>
                      <a
                        href={`/api/v1/workspace/mail/messages/${encodeURIComponent(
                          item.id,
                        )}/attachments/${index}`}
                        download
                      >
                        {file.name} ({file.bytes} bytes)
                      </a>
                    </li>
                  ))}
                </ul>
                <div className="mail-controls">
                  <Button
                    onClick={() => setSource({ id: item.id, action: 'reply' })}
                  >
                    Reply
                  </Button>
                  <Button
                    onClick={() =>
                      setSource({ id: item.id, action: 'reply-all' })
                    }
                  >
                    Reply all
                  </Button>
                  <Button
                    onClick={() =>
                      setSource({ id: item.id, action: 'forward' })
                    }
                  >
                    Forward
                  </Button>
                </div>
                <MessageControls
                  api={api}
                  id={item.id}
                  labels={item.labels}
                  onMove={onBack}
                />
              </article>
            </Row>
          ))}
        </Frame>
      )}
    </section>
  )
}
