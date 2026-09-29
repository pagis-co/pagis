// The `mail` block (ADR-0019). The daemon mints one for an
// inbound mail that woke the Agent and for a mail the Agent sent.
//
// It is one line in the frame, like the `call` strip: which way the
// mail went, who is on the other end, the subject, the mailbox that
// holds it and the tier of the sender. The strip opens the mail
// inspector, which reads the headers and the body live.
//
// The block carries no words of the message. What it does carry — the
// address and the subject of an inbound mail — is text a stranger
// wrote, so the tier chip stands beside it (ADR-0019).

import { Mail } from 'lucide-react'

import { Button } from '../primitives'
import { useMailInspector } from '../state/stores'
import { Card } from './Card'
import { TierChip } from './CallBlock'
import { subjectLine, whoLine, type MailBlockDto } from './mail'

export function MailBlock({ block }: { block: MailBlockDto }) {
  const open = useMailInspector((state) => state.open)
  return (
    <Card className="mail-block">
      <Button
        variant="ghost"
        shape="row"
        className="block-strip mail-strip"
        data-testid="mail-strip"
        onClick={() => open(block)}
      >
        <Mail size={16} aria-hidden focusable="false" />
        <span className="mail-strip-direction">
          {block.direction === 'inbound' ? 'In' : 'Out'}
        </span>
        <strong>{whoLine(block)}</strong>
        <span className="mail-strip-subject">
          {block.mailbox} · {subjectLine(block)}
        </span>
        {block.trust_tier != null && <TierChip tier={block.trust_tier} />}
        <span className="block-strip-open">Open</span>
      </Button>
    </Card>
  )
}
