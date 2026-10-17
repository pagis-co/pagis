import { useRef, useState } from 'react'
import { ArrowUp } from 'lucide-react'

import type { components } from '../../api/schema'
import { Button, IconButton, Textarea } from '../../primitives'
import '../Composer.css'

type ColleagueDto = components['schemas']['ColleagueDto']

export function ConversationComposer({
  onSend,
  participants = [],
  replyTo,
  onCancelReply,
}: {
  replyTo?: string
  onCancelReply?: () => void
  participants?: ColleagueDto[]
  onSend: (text: string, forPersonOnly: boolean) => void
}) {
  const input = useRef<HTMLTextAreaElement>(null)
  const [text, setText] = useState('')
  const [forPersonOnly, setForPersonOnly] = useState(false)
  const submit = () => {
    if (!text.trim()) return
    onSend(text.trim(), forPersonOnly)
    setForPersonOnly(false)
    setText('')
  }

  return (
    <form
      className="composer"
      onSubmit={(event) => {
        event.preventDefault()
        submit()
      }}
    >
      {replyTo && (
        <div role="status">
          Reply to {replyTo}
          <Button variant="ghost" size="sm" onClick={onCancelReply}>
            Cancel reply
          </Button>
        </div>
      )}
      <label>
        <input
          type="checkbox"
          checked={forPersonOnly}
          onChange={(event) => setForPersonOnly(event.target.checked)}
        />
        {' '}For the person only
      </label>
      <div aria-label="Mention a participant">
        {participants.map((participant) => (
          <Button
            key={participant.workspace_id}
            variant="ghost"
            size="sm"
            aria-label={`Mention ${participant.name}`}
            onClick={() => {
              setText((value) => `${value}${value && !value.endsWith(' ') ? ' ' : ''}@${participant.name} `)
              input.current?.focus()
            }}
          >
            @{participant.name}
          </Button>
        ))}
      </div>
      <div className="composer-row">
        <Textarea
          ref={input}
          className="composer-input"
          aria-label="Message"
          placeholder="Message"
          rows={1}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault()
              submit()
            }
          }}
        />
        <IconButton
          icon={ArrowUp}
          label="Send"
          type="submit"
          variant="primary"
          disabled={!text.trim()}
        />
      </div>
    </form>
  )
}
