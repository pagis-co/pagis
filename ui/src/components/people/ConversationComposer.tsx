import { useState } from 'react'
import { ArrowUp } from 'lucide-react'

import { IconButton, Textarea } from '../../primitives'
import '../Composer.css'

export function ConversationComposer({
  onSend,
}: {
  onSend: (text: string, forPersonOnly: boolean) => void
}) {
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
      <label><input type="checkbox" checked={forPersonOnly} onChange={event => setForPersonOnly(event.target.checked)} /> For the person only</label>
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
