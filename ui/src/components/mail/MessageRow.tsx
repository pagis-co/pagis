import { Circle } from 'lucide-react'

import type { components } from '../../api/schema'
import { Button } from '../../primitives'
import { formatMoment } from '../../timeline'

export type MailSummary = components['schemas']['WorkspaceMailSummaryDto']

export function MessageRow({
  message,
  selected,
  onOpen,
}: {
  message: MailSummary
  selected: boolean
  onOpen: () => void
}) {
  return (
    <Button
      shape="row"
      variant="ghost"
      className={`mail-list-row${message.seen ? '' : ' mail-unread'}`}
      aria-pressed={selected}
      onClick={onOpen}
    >
      {!message.seen && (
        <Circle aria-label="Unread" size={14} fill="currentColor" />
      )}
      <span className="mail-row-copy">
        <span className="mail-row-sender">{message.from}</span>
        <strong>{message.subject || '(No subject)'}</strong>
        {message.labels.length > 0 && (
          <span aria-label="Labels">{message.labels.join(', ')}</span>
        )}
        {message.triage && <span aria-label="Mail Triage note">{message.triage.note}</span>}
        <span className="mail-row-snippet">{message.snippet}</span>
      </span>
      <time dateTime={new Date(message.date).toISOString()}>
        {formatMoment(message.date)}
      </time>
    </Button>
  )
}
