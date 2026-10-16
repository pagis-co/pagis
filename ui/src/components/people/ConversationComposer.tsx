import { useState } from 'react'
import { ArrowUp } from 'lucide-react'

import { IconButton, Textarea } from '../../primitives'
import '../Composer.css'

export function ConversationComposer({
  onSend,
}: {
  onSend: (text: string) => void
}) {
  const [text, setText] = useState('')
  const submit = () => {
    if (!text.trim()) return
    onSend(text.trim())
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
      <div className="composer-row">
        <Textarea
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
