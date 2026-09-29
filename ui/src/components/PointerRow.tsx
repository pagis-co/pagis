// A derived DM pointer row: this DM's agent posted in another
// channel. The row links through to that channel.

import { Button } from '../primitives'
import { formatClock, type TimelineRow } from '../timeline'

import './MessageRow.css'
import './PointerRow.css'

export function PointerRow({
  row,
  onOpenChannel,
}: {
  row: TimelineRow
  onOpenChannel: (channelId: string) => void
}) {
  const pointer = row.pointer
  if (pointer === undefined) return null
  return (
    <div className="message message-pointer" data-testid="pointer-row">
      <Button
        size="sm"
        variant="link"
        className="pointer-link"
        onClick={() => onOpenChannel(pointer.channelId)}
      >
        Posted in {pointer.channelTitle ?? 'another channel'}
      </Button>
      <span className="pointer-preview">{row.text}</span>
      <span className="message-time">
        {formatClock(row.createdAt)}
      </span>
    </div>
  )
}
