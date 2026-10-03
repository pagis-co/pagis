// The timeline view model: server messages merged with the local
// pending sends, the live agent streams, and the live run progress.
// The server list is authoritative; a pending send shows until a
// server row with the same `pending_id` exists (the Mattermost
// reconcile pattern); a live stream overrides its row's
// text while it streams; a live progress line overrides the text
// of its `progress` block until the run ends. The same merge serves
// the channel timeline and a thread's replies.

import type { MessageDto, PointerDto, ReplyAuthorDto } from './api/client'
import type { LiveStream, RunProgress } from './state/stores'

export type SendState = 'sent' | 'pending' | 'failed'

/** One local optimistic send: `pending` until the daemon confirms. */
export interface PendingSend {
  pending_id: string
  text: string
  created_at: number
  state: 'pending' | 'failed'
}

/**
 * The scope key for pending sends and live streams: one channel's top
 * level, or one thread in it.
 */
export function threadScope(channelId: string, rootId?: string | null): string {
  return rootId == null ? channelId : `${channelId}/${rootId}`
}

/** A message with the timeline's thread rollup; replies carry none. */
export type TimelineMessage = MessageDto & {
  kind?: 'message'
  reply_count?: number
  last_reply_at?: number | null
  reply_authors?: ReplyAuthorDto[]
}

/** A derived DM pointer entry, as the timeline API returns it. */
export type PointerItem = PointerDto & { kind: 'pointer' }

/** One server timeline item: a message, or a derived DM pointer. */
export type TimelineInput = TimelineMessage | PointerItem

/** One rendered timeline row, oldest first. */
export interface TimelineRow {
  key: string
  /** A `pointer` row links to the agent's message elsewhere. */
  kind: 'message' | 'pointer'
  /** The link target of a pointer row. */
  pointer?: {
    channelId: string
    channelTitle: string | null
  }
  authorKind: string
  /** The agent who wrote it; the row names the speaker with it. */
  authorAgentId: string | null
  createdAt: number
  sendState: SendState
  /** The message status: `complete`, `streaming`, or `failed`. */
  status: string
  /** The run behind an agent reply; the cancel target. */
  runId: string | null
  /** The block array for confirmed rows; pending rows render text. */
  blocks: MessageDto['blocks']
  text: string
  /** When the reply settled; the end of the run behind it. */
  completedAt: number | null
  /** The thread rollup; 0 for replies and pending rows. */
  replyCount: number
  lastReplyAt: number | null
  /** The authors of the replies, the newest reply first. */
  replyAuthors: ReplyAuthor[]
}

/** One author of a thread's replies: the user, or one Agent. */
export interface ReplyAuthor {
  authorKind: string
  agentId: string | null
}

/**
 * The row's blocks with the run's live progress line in place of the
 * `progress` block's persisted text. Every other block is untouched,
 * and a row without a progress block keeps its own array, so the merge
 * allocates nothing for the common case.
 */
function liveProgress(
  blocks: MessageDto['blocks'],
  progress: RunProgress | undefined,
): MessageDto['blocks'] {
  if (progress === undefined) return blocks
  if (!blocks.some((block) => block.type === 'progress')) return blocks
  return blocks.map((block) =>
    block.type === 'progress' ? { ...block, text: progress.text } : block,
  )
}

/**
 * Merge one server page (newest first, as the API returns it) with the
 * scope's pending sends, live streams and run progress into ascending
 * rows. A pending send whose `pending_id` already has a server row is
 * dropped. A streaming row renders the live buffer; a live stream or a
 * live progress line without a server row yet renders as a synthetic
 * streaming row at the end.
 */
export function mergeTimeline(
  serverItems: TimelineInput[],
  pendingSends: PendingSend[],
  liveStreams: Record<string, LiveStream> = {},
  runProgress: Record<string, RunProgress> = {},
): TimelineRow[] {
  const messages = serverItems.filter(
    (item): item is TimelineMessage => item.kind !== 'pointer',
  )
  const confirmed = new Set<string>()
  for (const item of messages) {
    if (item.pending_id != null) confirmed.add(item.pending_id)
  }

  const rows: TimelineRow[] = serverItems
    .map((item) => {
      if (item.kind === 'pointer') {
        return {
          key: item.id,
          kind: 'pointer' as const,
          pointer: {
            channelId: item.channel_id,
            channelTitle: item.channel_title ?? null,
          },
          authorKind: 'agent',
          authorAgentId: item.agent_id,
          createdAt: item.created_at,
          sendState: 'sent' as SendState,
          status: 'complete',
          runId: null,
          blocks: [],
          text: item.preview,
          completedAt: null,
          replyCount: 0,
          lastReplyAt: null,
          replyAuthors: [],
        }
      }
      const live =
        item.status === 'streaming' ? liveStreams[item.id] : undefined
      const text = live?.text ?? item.text_content
      const progress =
        item.run_id == null ? undefined : runProgress[item.run_id]
      return {
        key: item.id,
        kind: 'message' as const,
        authorKind: item.author_kind,
        authorAgentId: item.author_agent_id ?? null,
        createdAt: item.created_at,
        sendState: 'sent' as SendState,
        status: item.status,
        runId: item.run_id ?? null,
        blocks:
          live !== undefined
            ? [{ type: 'markdown' as const, text }]
            : liveProgress(item.blocks, progress),
        text,
        completedAt: item.completed_at ?? null,
        replyCount: item.reply_count ?? 0,
        lastReplyAt: item.last_reply_at ?? null,
        replyAuthors: (item.reply_authors ?? []).map((author) => ({
          authorKind: author.author_kind,
          agentId: author.author_agent_id ?? null,
        })),
      }
    })
    .reverse()

  for (const send of pendingSends) {
    if (confirmed.has(send.pending_id)) continue
    rows.push({
      key: send.pending_id,
      kind: 'message',
      authorKind: 'user',
      authorAgentId: null,
      createdAt: send.created_at,
      sendState: send.state,
      status: 'complete',
      runId: null,
      blocks: [{ type: 'markdown' as const, text: send.text }],
      text: send.text,
      completedAt: null,
      replyCount: 0,
      lastReplyAt: null,
      replyAuthors: [],
    })
  }

  // A run the timeline has no progress row for yet (the refetch races
  // the first frame): render the daemon's line as a streaming row.
  const known = new Set(serverItems.map((item) => item.id))
  for (const progress of Object.values(runProgress)) {
    if (known.has(progress.message_id)) continue
    rows.push({
      key: progress.message_id,
      kind: 'message',
      authorKind: 'agent',
      authorAgentId: progress.agent_id,
      createdAt: Date.now(),
      sendState: 'sent',
      status: 'streaming',
      runId: progress.run_id,
      blocks: [
        { type: 'progress' as const, run_id: progress.run_id, text: progress.text },
      ],
      text: progress.text,
      completedAt: null,
      replyCount: 0,
      lastReplyAt: null,
      replyAuthors: [],
    })
  }

  // A live stream the timeline has no row for yet (the refetch races
  // the first delta): render it as a streaming agent row.
  for (const live of Object.values(liveStreams)) {
    if (known.has(live.message_id)) continue
    rows.push({
      key: live.message_id,
      kind: 'message',
      authorKind: 'agent',
      authorAgentId: live.agent_id,
      createdAt: Date.now(),
      sendState: 'sent',
      status: 'streaming',
      runId: live.run_id,
      blocks: [{ type: 'markdown' as const, text: live.text }],
      text: live.text,
      completedAt: null,
      replyCount: 0,
      lastReplyAt: null,
      replyAuthors: [],
    })
  }
  return rows
}

// The conversation view model: the merged rows read as a
// conversation, not a log. Consecutive messages from one author group
// under one header, the agent's tool loop folds into one work row under
// the reply it produced, and the day, the unread point and the system
// events become strips of their own.

/** How long one author keeps writing under one header. */
export const GROUP_WINDOW_MS = 5 * 60 * 1000

/** The run behind one reply, as the folded work row states it. */
export interface WorkSummary {
  runId: string
  /** The first progress line of the run: the start of the work. */
  startedAt: number
  /** When the reply settled, or when it was written. */
  endedAt: number
  /** The terminal progress line: `Done`, `Failed` or `Stopped`. */
  outcome: string
}

/** One rendered entry of the conversation, oldest first. */
export type ConversationItem =
  | { kind: 'day'; key: string; at: number }
  | { kind: 'unread'; key: string }
  | { kind: 'system'; key: string; row: TimelineRow }
  | { kind: 'pointer'; key: string; row: TimelineRow }
  /** The Run that works now: the live row at the end of the column. */
  | { kind: 'working'; key: string; row: TimelineRow }
  /** Memory settlement after the reply: a quiet live row. */
  | { kind: 'reflecting'; key: string; row: TimelineRow }
  /** A Run that ended with no reply: a quiet Stopped or Failed line. */
  | { kind: 'outcome'; key: string; row: TimelineRow; outcome: RunOutcome }
  | {
      kind: 'message'
      key: string
      row: TimelineRow
      /** The author already has a header above, so this row draws none. */
      grouped: boolean
      /** The run behind the reply, folded under it. */
      work: WorkSummary | null
    }

/**
 * A row the agent's tool loop wrote for itself: every block is the run's
 * `progress` block. It carries no reply, only the loop's state
 * line ("Thinking…", "Running `git status`…", "Done").
 */
export function isProgressRow(row: TimelineRow): boolean {
  return (
    row.kind === 'message' &&
    row.blocks.length > 0 &&
    row.blocks.every((block) => block.type === 'progress')
  )
}

/** The line a completed tool loop leaves behind, with nothing to say. */
const SILENT_OUTCOME = 'Done'

/** How a Run ended when it ended badly. The daemon writes the word as
 *  the terminal progress line (`pagis-agent/src/progress.rs`). */
export type RunOutcome = 'Failed' | 'Stopped'

/** The bad end a progress line names, or null for every other line. */
export function runOutcome(text: string): RunOutcome | null {
  if (text === 'Failed' || text === 'Stopped') return text
  return null
}

/** The identity one group header stands for. */
function authorOf(row: TimelineRow): string {
  return `${row.authorKind}:${row.authorAgentId ?? ''}`
}

/** The local calendar day the divider groups by. */
export function dayKey(at: number): string {
  return new Date(at).toDateString()
}

/** `51 s`, or `1 min 4 s`. */
export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000))
  if (seconds < 60) return `${seconds} s`
  return `${Math.floor(seconds / 60)} min ${seconds % 60} s`
}

/** The time on a timeline row: `3:06 PM` in the reader's locale. One
 *  definition, so every row, chip and panel in a conversation agrees.
 *  Seconds say nothing a reader acts on and make the column noisy. */
export function formatClock(at: number): string {
  return new Date(at).toLocaleTimeString(undefined, {
    hour: 'numeric',
    minute: '2-digit',
  })
}

/** A moment that can fall on another day: `3:06 PM` on the day of
 *  `now`, and `Sep 29, 3:06 PM` on any other day. */
export function formatMoment(at: number, now: number = Date.now()): string {
  if (dayKey(at) === dayKey(now)) return formatClock(at)
  const day = new Date(at).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
  return `${day}, ${formatClock(at)}`
}

/**
 * Fold the merged rows into the conversation the timeline renders.
 *
 * A progress row is plumbing once its run ends: it joins the reply of
 * its own run as a `work` summary and renders no row of its own. While
 * the run works, its live line follows the latest reply of the run, so
 * Stop stays in reach. A progress row with no reply stays visible while
 * it says something — the live line, a failure, a cancel — and goes
 * when it only says `Done`.
 *
 * `lastReadAt` puts the unread marker before the first row written after
 * it; no marker shows when every row is read.
 */
export function buildConversation(
  rows: TimelineRow[],
  options: { lastReadAt?: number | null } = {},
): ConversationItem[] {
  const replyOfRun = new Map<string, TimelineRow>()
  const lastReplyOfRun = new Map<string, TimelineRow>()
  for (const row of rows) {
    if (row.kind !== 'message' || isProgressRow(row) || row.runId === null) {
      continue
    }
    if (!replyOfRun.has(row.runId)) replyOfRun.set(row.runId, row)
    lastReplyOfRun.set(row.runId, row)
  }

  // The live line of a run that already replied: the Working row, or
  // the memory settlement, under the latest reply of the run.
  const liveOfRun = new Map<string, TimelineRow>()
  for (const row of rows) {
    if (
      isProgressRow(row) &&
      row.runId !== null &&
      row.text !== SILENT_OUTCOME &&
      runOutcome(row.text) === null &&
      replyOfRun.has(row.runId)
    ) {
      liveOfRun.set(row.runId, row)
    }
  }

  const work = new Map<string, WorkSummary>()
  for (const row of rows) {
    if (!isProgressRow(row) || row.runId === null) continue
    if (liveOfRun.has(row.runId)) continue
    const reply = replyOfRun.get(row.runId)
    if (reply === undefined) continue
    // The progress row spans the run: the daemon opens it when the run
    // starts and settles it when the run ends.
    const started = work.get(reply.key)?.startedAt ?? row.createdAt
    work.set(reply.key, {
      runId: row.runId,
      startedAt: Math.min(started, row.createdAt),
      endedAt: row.completedAt ?? reply.completedAt ?? reply.createdAt,
      outcome: row.text,
    })
  }

  const lastReadAt = options.lastReadAt ?? null
  const items: ConversationItem[] = []
  let day: string | null = null
  let previousAuthor: string | null = null
  let previousAt = 0
  let unreadPlaced = lastReadAt === null

  for (const row of rows) {
    // A progress row with a reply is plumbing, and a `Done` row with
    // none has nothing left to say.
    const reflecting =
      isProgressRow(row) &&
      row.text === 'Updating memory' &&
      (row.runId === null || !replyOfRun.has(row.runId))
    const loop =
      isProgressRow(row) &&
      row.runId !== null &&
      (!replyOfRun.has(row.runId) || reflecting)
    if (isProgressRow(row) && !loop) continue
    if (loop && row.text === SILENT_OUTCOME) continue

    const rowDay = dayKey(row.createdAt)
    if (rowDay !== day) {
      day = rowDay
      previousAuthor = null
      items.push({ kind: 'day', key: `day-${rowDay}`, at: row.createdAt })
    }
    if (!unreadPlaced && lastReadAt !== null && row.createdAt > lastReadAt) {
      unreadPlaced = true
      previousAuthor = null
      items.push({ kind: 'unread', key: 'unread' })
    }

    if (loop) {
      previousAuthor = null
      const ended = runOutcome(row.text)
      if (reflecting) {
        items.push({ kind: 'reflecting', key: row.key, row })
      } else {
        items.push(
          ended === null
            ? { kind: 'working', key: row.key, row }
            : { kind: 'outcome', key: row.key, row, outcome: ended },
        )
      }
      continue
    }
    if (row.kind === 'pointer') {
      previousAuthor = null
      items.push({ kind: 'pointer', key: row.key, row })
      continue
    }
    if (row.authorKind === 'system') {
      previousAuthor = null
      items.push({ kind: 'system', key: row.key, row })
      continue
    }

    const author = authorOf(row)
    const grouped =
      author === previousAuthor && row.createdAt - previousAt <= GROUP_WINDOW_MS
    previousAuthor = author
    previousAt = row.createdAt
    items.push({
      kind: 'message',
      key: row.key,
      row,
      grouped,
      work: work.get(row.key) ?? null,
    })
    if (row.runId !== null && lastReplyOfRun.get(row.runId) === row) {
      const live = liveOfRun.get(row.runId)
      if (live !== undefined) {
        previousAuthor = null
        items.push({
          kind: live.text === 'Updating memory' ? 'reflecting' : 'working',
          key: live.key,
          row: live,
        })
      }
    }
  }
  return items
}

/** The prose of a message, for a place that shows one clamped line and
 *  cannot render blocks: a preview reads what the writer wrote, not the
 *  Markdown that carries it. Emphasis, code ticks, headings, list
 *  bullets, quotes and link syntax go; the link's words stay. */
export function plainText(markdown: string): string {
  return markdown
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/^\s{0,3}#{1,6}\s+/gm, '')
    .replace(/^\s{0,3}>\s?/gm, '')
    .replace(/^\s*(?:[-*+]|\d+\.)\s+/gm, '')
    .replace(/(\*\*|__)(.*?)\1/g, '$2')
    .replace(/(\*|_)(.*?)\1/g, '$2')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/\s+/g, ' ')
    .trim()
}
