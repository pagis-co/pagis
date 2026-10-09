// System messages keep their interactive records. Plain notices use
// a compact row without an author header. A record at the top level,
// such as a call or a coding session, is the root of its Thread, so it
// shows the replies chip and "Reply in thread" as a message does.

import { Info, MessageSquare } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { Blocks } from '../blocks/BlockView'
import { IconButton } from '../primitives'
import { formatClock, type TimelineRow } from '../timeline'
import { RepliesChip } from './RepliesChip'

import './SystemRow.css'

export function SystemRow({
  row,
  api,
  onOpenThread,
}: {
  row: TimelineRow
  api: ApiClient
  /** Present in the channel timeline only; the record opens its
   *  thread. */
  onOpenThread?: (rootId: string) => void
}) {
  // Calls, mail and other records keep their controls and full content.
  const structured = row.blocks.some((block) => block.type !== 'markdown')
  if (structured) {
    return (
      <div className="system-record" data-testid="system-record">
        <Blocks blocks={row.blocks} api={api} />
        {onOpenThread !== undefined && (
          <>
            {row.replyCount > 0 && (
              <RepliesChip api={api} row={row} onOpen={() => onOpenThread(row.key)} />
            )}
            <div className="system-record-tools">
              <IconButton
                icon={MessageSquare}
                label="Reply in thread"
                variant="ghost"
                size="sm"
                onClick={() => onOpenThread(row.key)}
              />
            </div>
          </>
        )}
      </div>
    )
  }
  return (
    <div className="system-row" data-testid="system-row">
      <Info size={14} aria-hidden focusable="false" />
      <span className="system-row-text">{row.text}</span>
      <span className="system-row-time">
        {formatClock(row.createdAt)}
      </span>
    </div>
  )
}
