// The Thread: the conversation in the 680 px reading column.
// The virtualized list (react-virtuoso) keeps variable-height rows and
// follows new messages at the bottom. The merged rows are folded into a
// conversation first: the work record under the reply it
// produced, the Working row at the end, the quiet Stopped and Failed
// lines, the Today and New dividers, and the system strips. Rows open
// their reply thread in the thread pane.

import { useMemo, useRef, useState } from 'react'
import { ArrowDown, MessageSquare, Search } from 'lucide-react'
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso'

import type { ApiClient } from '../api/client'
import { Button, ReadingColumn } from '../primitives'
import {
  useAgentNames,
  useAgents,
  useChannels,
  useSendMessage,
  useTimeline,
} from '../queries'
import { createSpeaker } from '../speech'
import { useIsMobile } from '../state/useIsMobile'
import { useComposerDraft } from '../state/composerDraft'
import {
  selectLiveStreams,
  selectPendingSends,
  selectRunProgress,
  selectThreadQuery,
  useDeskFocus,
  useLiveStreams,
  usePendingSends,
  useRunProgress,
  useSpeaking,
  useThreadSearch,
} from '../state/stores'
import {
  buildConversation,
  mergeTimeline,
  threadScope,
  type ConversationItem,
} from '../timeline'
import { MessageRow } from './MessageRow'
import { OutcomeLine } from './OutcomeLine'
import { PageState } from './PageState'
import { PointerRow } from './PointerRow'
import { SystemRow } from './SystemRow'
import { WorkingRow } from './WorkingRow'

import './Timeline.css'

/** The day one strip of messages was written on. */
function dayLabel(at: number): string {
  const day = new Date(at)
  const today = new Date()
  if (day.toDateString() === today.toDateString()) return 'Today'
  const yesterday = new Date(today.getTime() - 24 * 60 * 60 * 1000)
  if (day.toDateString() === yesterday.toDateString()) return 'Yesterday'
  return day.toLocaleDateString(undefined, {
    weekday: 'long',
    month: 'long',
    day: 'numeric',
  })
}

/** The items a search keeps: the messages that carry the words. The
 *  dividers and the live rows say nothing a reader searches for. */
function searchItems(items: ConversationItem[], query: string): ConversationItem[] {
  const words = query.trim().toLowerCase()
  if (words === '') return items
  return items.filter(
    (item) => item.kind === 'message' && item.row.text.toLowerCase().includes(words),
  )
}

/** The question a Run answered: your last message above the item. */
function questionBefore(items: ConversationItem[], index: number): string | null {
  for (let at = index - 1; at >= 0; at -= 1) {
    const item = items[at]
    if (item.kind === 'message' && item.row.authorKind === 'user') return item.row.text
  }
  return null
}

export function Timeline({
  api,
  channelId,
  onOpenThread,
  onOpenChannel,
  onOpenDesk,
}: {
  api: ApiClient
  channelId: string
  onOpenThread: (rootId: string) => void
  /** A pointer row opens the channel it links to. */
  onOpenChannel: (channelId: string) => void
  /** Opens the Desk panel. */
  onOpenDesk: () => void
}) {
  const phone = useIsMobile()
  const timeline = useTimeline(api, channelId)
  const pending = usePendingSends(selectPendingSends(channelId))
  const live = useLiveStreams(selectLiveStreams(channelId))
  const progress = useRunProgress(selectRunProgress(channelId))
  const query = useThreadSearch(selectThreadQuery(channelId))
  const send = useSendMessage(api, channelId)
  const agentNames = useAgentNames(api)
  const agents = useAgents(api)
  const channels = useChannels(api)
  const channel = (channels.data ?? []).find((item) => item.id === channelId)
  const dmPartner =
    channel?.kind === 'dm' && channel.agent_ids.length === 1
      ? agentNames[channel.agent_ids[0]]
      : undefined
  const scroller = useRef<VirtuosoHandle | null>(null)
  const [atBottom, setAtBottom] = useState(true)

  const rows = useMemo(
    () => mergeTimeline(timeline.data ?? [], pending, live, progress),
    [timeline.data, pending, live, progress],
  )

  // The read point of this visit: what the channel already held when it
  // opened. Every later message falls under the New divider. The
  // daemon keeps no read state, so the visit is the only source.
  const readMark = useRef<{ channel: string; at: number | null }>({
    channel: channelId,
    at: null,
  })
  if (readMark.current.channel !== channelId) {
    readMark.current = { channel: channelId, at: null }
  }
  if (readMark.current.at === null && rows.length > 0) {
    readMark.current.at = rows[rows.length - 1].createdAt
  }

  const conversation = useMemo(
    () => buildConversation(rows, { lastReadAt: readMark.current.at }),
    [rows],
  )
  const items = useMemo(() => searchItems(conversation, query), [conversation, query])

  // Speech on demand: the hover tools read one reply aloud.
  const speaker = useMemo(
    () =>
      createSpeaker((channel, messageId) =>
        api
          .GET('/api/v1/channels/{channel_id}/messages/{message_id}', {
            params: { path: { channel_id: channel, message_id: messageId } },
          })
          .then(({ data }) => {
            if (data === undefined) throw new Error('message not found')
            return data
          }),
      ),
    [api],
  )

  if (timeline.isPending) {
    return <div className="timeline-empty"><PageState icon={MessageSquare} title="Loading conversation…" /></div>
  }
  if (timeline.isError) {
    return <div className="timeline-empty"><PageState icon={MessageSquare} title="Could not load this conversation" onRetry={() => { void timeline.refetch() }}>
      Your messages could not be retrieved. Try again when Pagis is connected.
    </PageState></div>
  }
  if (items.length === 0 && query.trim() !== '') {
    return (
      <div className="timeline-empty">
        <PageState icon={Search} title="No message matches">
          No message in this conversation carries these words.
        </PageState>
      </div>
    )
  }
  if (items.length === 0) {
    return (
      <div className="timeline-empty">
        <PageState icon={MessageSquare} title={dmPartner === undefined
          ? 'No messages yet. Say hello.'
          : `Say hello to ${dmPartner}`}>
          {channel?.kind === 'group'
            ? 'Use @mentions to ask a sprite to join the conversation.'
            : 'Start with a question, a task, or an idea.'}
        </PageState>
      </div>
    )
  }

  const agentName = (agentId: string | null) =>
    (agentId === null ? undefined : agentNames[agentId]) ?? 'Sprite'
  const agentAppearance = (agentId: string | null) =>
    agents.data?.find((agent) => agent.id === agentId)?.avatar

  const renderItem = (index: number, item: ConversationItem) => {
    if (item.kind === 'day') {
      return (
        <div className="timeline-divider" data-testid="day-divider">
          {dayLabel(item.at)}
        </div>
      )
    }
    if (item.kind === 'unread') {
      return (
        <div className="timeline-divider timeline-divider-new" data-testid="unread-marker">
          New
        </div>
      )
    }
    if (item.kind === 'system') {
      return <SystemRow row={item.row} api={api} />
    }
    if (item.kind === 'pointer') {
      return <PointerRow row={item.row} onOpenChannel={onOpenChannel} />
    }
    if (item.kind === 'working' || item.kind === 'reflecting') {
      return (
        <WorkingRow
          api={api}
          row={item.row}
          agentName={agentName(item.row.authorAgentId)}
          agentAppearance={agentAppearance(item.row.authorAgentId)}
          onOpenDesk={onOpenDesk}
        />
      )
    }
    if (item.kind === 'outcome') {
      const question = questionBefore(items, index)
      return (
        <OutcomeLine
          api={api}
          row={item.row}
          outcome={item.outcome}
          authorName={agentName(item.row.authorAgentId)}
          authorAppearance={agentAppearance(item.row.authorAgentId)}
          onAskAgain={() => {
            if (question !== null) {
              useComposerDraft.getState().set(threadScope(channelId), question)
            }
          }}
        />
      )
    }
    return (
      <MessageRow
        api={api}
        row={item.row}
        grouped={item.grouped}
        work={item.work}
        authorAppearance={agentAppearance(item.row.authorAgentId)}
        authorName={
          item.row.authorAgentId === null
            ? undefined
            : agentNames[item.row.authorAgentId]
        }
        onRetry={(pendingId, text) => send.mutate({ pendingId, text })}
        onOpenThread={onOpenThread}
        onSpeak={(messageId) => {
          void speaker
            .speak(channelId, messageId)
            .then(() => useSpeaking.getState().markSpoken(messageId))
            .catch(() => undefined)
        }}
        onShowScreenshot={(screenshotId) => {
          useDeskFocus.getState().show(screenshotId)
          onOpenDesk()
        }}
      />
    )
  }

  return (
    <div className="timeline-wrap">
      <Virtuoso
        ref={scroller}
        className="timeline"
        alignToBottom={phone}
        data={items}
        computeItemKey={(_index, item) => item.key}
        initialTopMostItemIndex={items.length - 1}
        followOutput="smooth"
        atBottomStateChange={setAtBottom}
        itemContent={(index, item) => (
          <div className="timeline-item">
            <ReadingColumn>{renderItem(index, item)}</ReadingColumn>
          </div>
        )}
      />
      {!atBottom && (
        <Button
          size="sm"
          shape="pill"
          variant="outline"
          className="timeline-jump"
          onClick={() =>
            scroller.current?.scrollToIndex({
              index: items.length - 1,
              behavior: 'smooth',
              align: 'end',
            })
          }
        >
          <ArrowDown size={14} aria-hidden focusable="false" />
          Jump to latest
        </Button>
      )}
    </div>
  )
}
