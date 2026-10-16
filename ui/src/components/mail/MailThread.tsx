import { Capacitor } from '@capacitor/core'
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
import { formatBytes } from '../../blocks/BlockView'
import { useIsMobile } from '../../primitives/useMediaQuery'
import { NavBar } from '../phone/TopBar'
import { openAttachment } from './openAttachment'

export function MailThread({
  api,
  messageId,
  onBack,
}: {
  api: ApiClient
  messageId: string
  onBack: () => void
}) {
  const phone = useIsMobile()
  const native = phone && Capacitor.isNativePlatform()
  const [attachmentError, setAttachmentError] = useState(false)
  const [source, setSource] = useState<MailSource>()
  const message = useWorkspaceMailMessage(api, messageId)
  const thread = useWorkspaceMailThread(api, message.data?.thread_id)

  return (
    <section className="mail-thread" aria-label="Mail thread">
      {phone ? (
        <NavBar back={{ label: 'Mail', onBack }} />
      ) : <Button onClick={onBack}>Back to mail</Button>}
      {attachmentError && <p role="alert">Could not open the attachment. Try again.</p>}
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
                <details>
                  <summary>
                    Spam score {item.spam_verdict.score} · {item.spam_verdict.action}
                  </summary>
                  <ul>
                    {item.spam_verdict.reasons.map((reason, index) => (
                      <li key={index}>{reason}</li>
                    ))}
                  </ul>
                </details>
                <pre>{item.text}</pre>
                <ul className="mail-attachments">
                  {item.attachments?.map((file, index) => {
                    const url = `/api/v1/workspace/mail/messages/${encodeURIComponent(item.id)}/attachments/${index}`
                    const label = `${file.name} · ${formatBytes(file.bytes)}`
                    return (
                      <li key={index}>
                        {native ? (
                          <Button onClick={() => {
                            setAttachmentError(false)
                            void openAttachment(url, file.name).catch(() => setAttachmentError(true))
                          }}>{label}</Button>
                        ) : (
                          <a
                            href={url}
                            download={phone ? undefined : true}
                            target={phone ? '_blank' : undefined}
                            rel={phone ? 'noopener' : undefined}
                          >
                            {label}
                          </a>
                        )}
                      </li>
                    )
                  })}
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
