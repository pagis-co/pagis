// One message of the Thread. Your message is a bubble at the
// right with the time under it. The Agent speaks in prose under a name
// line, with no bubble, so a long answer reads as writing and not as
// chat. The work record of the Run that wrote the reply folds under
// it, and the hover tools carry the four acts on one message.

import { Link } from '@tanstack/react-router'
import { Copy, MessageSquare, SquareArrowOutUpRight, Volume2 } from 'lucide-react'
import { useState } from 'react'

import type { ApiClient } from '../api/client'
import { Avatar, Button, IconButton } from '../primitives'
import { Blocks } from '../blocks/BlockView'
import { useSpeaking } from '../state/stores'
import { useIsMobile } from '../state/useIsMobile'
import { formatClock, type TimelineRow, type WorkSummary } from '../timeline'
import { LearnedLine } from './LearnedLine'
import { RepliesChip } from './RepliesChip'
import { WorkRecord } from './WorkRecord'

import './MessageRow.css'

/** The label of a row whose author has no name of its own. */
const AUTHOR_LABEL: Record<string, string> = {
  user: 'You',
  agent: 'Sprite',
  system: 'System',
}

/** How recent a message must be for its row to fade up. */
const FRESH_MS = 5_000

/** Put the row's text on the clipboard; a browser that denies it does
 *  nothing, because the row has no place to report the refusal. A page
 *  that is not a secure context, such as an http:// address on another
 *  machine, has no Clipboard API, so the copy goes through a selected
 *  text area and the copy command, which every page has. */
function copyText(text: string): void {
  if (navigator.clipboard !== undefined) {
    void navigator.clipboard.writeText(text).catch(() => undefined)
    return
  }
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.append(area)
  area.select()
  document.execCommand('copy')
  area.remove()
}

export function MessageRow({
  api,
  row,
  authorName,
  authorAppearance,
  grouped = false,
  work = null,
  onRetry,
  onOpenThread,
  onSpeak,
  onShowScreenshot,
}: {
  api: ApiClient
  row: TimelineRow
  /** The agent's name; absent until the roster resolves it. */
  authorName?: string
  authorAppearance?: import('../avatars/catalog').SpriteAppearance
  /** The author already has a name line above this row. */
  grouped?: boolean
  /** The Run behind an agent reply, folded under it. */
  work?: WorkSummary | null
  onRetry: (pendingId: string, text: string) => void
  /** Present in the channel timeline only; a confirmed row opens its
   *  thread. */
  onOpenThread?: (rootId: string) => void
  /** Reads this reply aloud; absent where speech has no source. */
  onSpeak?: (messageId: string) => void
  /** Scrolls the Desk panel to the screen one step took. */
  onShowScreenshot?: (screenshotId: string) => void
}) {
  const phone = useIsMobile()
  const mine = row.authorKind === 'user'
  const streaming = row.status === 'streaming'
  const canOpenThread = onOpenThread !== undefined && row.sendState === 'sent'
  // The small mark that says a reply was spoken; the audio
  // itself is never kept.
  const spoken = useSpeaking((state) => state.spoken[row.key] === true)
  const name = authorName ?? AUTHOR_LABEL[row.authorKind] ?? row.authorKind
  const time = phone
    ? new Date(row.createdAt).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' })
    : formatClock(row.createdAt)
  // The arrival fade. The row asks once, when it mounts: a row
  // written moments ago arrived while the reader watched, so it fades
  // up. The virtualizer mounts an old row again on every pass through
  // the history, and that row must not move.
  const [fresh] = useState(() => Date.now() - row.createdAt < FRESH_MS)
  // The replies chip opens the reply thread in the right panel.
  const replies = canOpenThread && row.replyCount > 0 && (
    <RepliesChip api={api} row={row} onOpen={() => onOpenThread(row.key)} />
  )
  const tools = (
    <div className="message-tools" data-testid="message-actions">
      {canOpenThread && (
        <IconButton
          icon={MessageSquare}
          label="Reply in thread"
          variant="ghost"
          size="sm"
          onClick={() => onOpenThread(row.key)}
        />
      )}
      <IconButton
        icon={Copy}
        label="Copy"
        variant="ghost"
        size="sm"
        onClick={() => copyText(row.text)}
      />
      {onSpeak !== undefined && !mine && (
        <IconButton
          icon={Volume2}
          label="Read aloud"
          variant="ghost"
          size="sm"
          onClick={() => onSpeak(row.key)}
        />
      )}
      {row.runId != null && (
        // The work record is a URL, and the router owns it:
        // the Run opens in place, without a reload.
        <Link
          to="/runs/$runId"
          params={{ runId: row.runId }}
          className="message-run"
          aria-label="Open the Run"
          title="Open the Run"
        >
          <SquareArrowOutUpRight size={14} aria-hidden focusable="false" />
        </Link>
      )}
    </div>
  )

  if (mine) {
    return (
      <div
        className={`message message-mine message-${row.sendState}${fresh ? ' message-fresh' : ''}`}
        data-testid="message-row"
        data-grouped={grouped}
      >
        <div className="message-bubble">
          <Blocks blocks={row.blocks} api={api} />
        </div>
        <div className="message-under">
          {row.sendState === 'pending' ? (
            <span className="message-state">sending…</span>
          ) : row.sendState === 'failed' ? (
            <Button
              size="sm"
              variant="link"
              className="message-retry"
              onClick={() => onRetry(row.key, row.text)}
            >
              Failed — retry
            </Button>
          ) : (
            !grouped && <span className="message-time">{time}</span>
          )}
        </div>
        {replies}
        {tools}
      </div>
    )
  }

  return (
    <div
      className={`message message-said${grouped ? ' message-grouped' : ''}${fresh ? ' message-fresh' : ''}`}
      data-testid="message-row"
      data-grouped={grouped}
    >
      {!grouped && (
        <Avatar
          id={row.authorAgentId ?? row.authorKind}
          name={name}
          appearance={authorAppearance}
          size="md"
          className="message-face"
        />
      )}
      <div className="message-body">
        {!grouped && (
          <div className="message-name">
            <span className="message-author">{name}</span>
            <span className="message-time">{time}</span>
            {spoken && <span className="message-spoken">spoken</span>}
          </div>
        )}
        <div className="message-prose">
          <Blocks blocks={row.blocks} api={api} />
          {streaming && <span className="message-streaming-cursor" aria-hidden />}
        </div>
        {work !== null && (
          <WorkRecord api={api} work={work} onShowScreenshot={onShowScreenshot} />
        )}
        {row.runId !== null && <LearnedLine api={api} messageId={row.key} authorName={name} />}
        {replies}
      </div>
      {tools}
    </div>
  )
}
