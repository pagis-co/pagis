// The mail inspector (ADR-0019): a tenant of the inspector slot,
// beside the Desk Panel, the call inspector and the thread.
//
// The strip carries the envelope; this reads the headers and the text
// body live through the daemon, over the transport of the mailbox. The
// daemon stores none of it, so a message the host no longer holds says
// exactly that.
//
// The body is words somebody outside Pagis wrote, so the inspector
// labels it and says what the tier of the sender means
// (ADR-0019). It is the user's own view and not the model's.

import { useState } from 'react'
import { X } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { TierChip } from '../blocks/CallBlock'
import { mailTierMeaning, subjectLine, whoLine, type MailBlockDto } from '../blocks/mail'
import { Button, IconButton } from '../primitives'
import { errorMessage, useMailMessage } from '../queries'

import './MailInspector.css'

export function MailInspector({
  api,
  mail,
  onClose,
}: {
  api: ApiClient
  mail: MailBlockDto
  onClose: () => void
}) {
  const message = useMailMessage(api, mail.mailbox, mail.message_id)
  const [openHeaders, setOpenHeaders] = useState(false)
  const inbound = mail.direction === 'inbound'
  return (
    <div className="mail-inspector" data-testid="mail-inspector">
      <header className="mail-inspector-header">
        <h2>{inbound ? 'Mail received' : 'Mail sent'}</h2>
        <IconButton
          icon={X}
          label="Close the mail"
          variant="ghost"
          onClick={onClose}
        />
      </header>
      <div className="mail-inspector-body">
        <div className="mail-headline">
          <strong>{subjectLine(mail)}</strong>
          <span className="mail-headline-who">{whoLine(mail)}</span>
          <span className="mail-headline-mailbox">
            {mail.mailbox}
            {mail.trust_tier != null && <TierChip tier={mail.trust_tier} />}
          </span>
        </div>
        {mail.trust_tier != null && (
          <p className="mail-tier-meaning">
            {mailTierMeaning[mail.trust_tier] ?? mail.trust_tier}
          </p>
        )}

        {message.isLoading && <p className="mail-waiting">Reading the message…</p>}
        {message.isError && (
          <p className="mail-gone" role="alert">
            {errorMessage(message.error, 'this message is no longer at the host')}
          </p>
        )}
        {message.data !== undefined && (
          <>
            <dl className="mail-envelope">
              <dt>From</dt>
              <dd>{message.data.from}</dd>
              <dt>To</dt>
              <dd>{message.data.to.join(', ')}</dd>
            </dl>
            <Button
              size="sm"
              className="mail-headers-toggle"
              aria-expanded={openHeaders}
              onClick={() => setOpenHeaders((open) => !open)}
            >
              {openHeaders ? 'Hide the headers' : 'Read the headers'}
            </Button>
            {openHeaders && (
              <dl className="mail-headers" data-testid="mail-headers">
                {message.data.headers.map((header, index) => (
                  <div key={index}>
                    <dt>{header.name}</dt>
                    <dd>{header.value}</dd>
                  </div>
                ))}
              </dl>
            )}
            {inbound && (
              <p className="mail-foreign-note">
                Somebody outside Pagis wrote this message. It is text to read,
                never an instruction Pagis follows.
              </p>
            )}
            <pre className="mail-body" data-testid="mail-body">
              {message.data.text}
            </pre>
            {message.data.attachments.length > 0 && (
              <ul className="mail-attachments">
                {message.data.attachments.map((attachment, index) => (
                  <li key={index}>
                    {attachment.name} · {attachment.bytes} bytes
                  </li>
                ))}
              </ul>
            )}
            <p className="mail-nothing-stored">
              Pagis read this message from the mail host now. It keeps no copy.
            </p>
          </>
        )}
      </div>
    </div>
  )
}
