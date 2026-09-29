// System messages keep their interactive records. Plain notices use
// a compact row without an author header.

import { Info } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { Blocks } from '../blocks/BlockView'
import { formatClock, type TimelineRow } from '../timeline'

import './SystemRow.css'

export function SystemRow({ row, api }: { row: TimelineRow; api: ApiClient }) {
  // Calls, mail and other records keep their controls and full content.
  const structured = row.blocks.some((block) => block.type !== 'markdown')
  if (structured) {
    return (
      <div className="system-record" data-testid="system-record">
        <Blocks blocks={row.blocks} api={api} />
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
