// The thread pane: the root message, its replies, and a composer
// that sends into the thread. Replies reuse the timeline merge, so
// pending sends and live agent streams render the same way as the
// channel timeline. Threads are short; no virtualization. The pane
// sits over the Desk panel: the header counts the replies and
// carries the way back to the Desk and the close.

import { useMemo } from 'react'
import { Monitor, X } from 'lucide-react'

import type { ApiClient } from '../api/client'
import {
  useAgentNames,
  useAgents,
  useCancelRun,
  useSendMessage,
  useThread,
} from '../queries'
import {
  selectLiveStreams,
  selectPendingSends,
  selectRunProgress,
  useLiveStreams,
  usePendingSends,
  useRunProgress,
} from '../state/stores'
import { Button, IconButton } from '../primitives'
import {
  formatClock,
  isProgressRow,
  mergeTimeline,
  plainText,
  threadScope,
  type TimelineRow,
} from '../timeline'
import { ChannelComposer } from './composer/ChannelComposer'
import { MessageRow } from './MessageRow'
import { WorkingRow } from './WorkingRow'

import './ThreadPane.css'
import './Timeline.css'

export function ThreadPane({
  api,
  channelId,
  rootId,
  onClose,
  onOpenDesk,
}: {
  api: ApiClient
  channelId: string
  rootId: string
  onClose: () => void
  /** Puts the Desk panel back in the slot. */
  onOpenDesk: () => void
}) {
  const thread = useThread(api, channelId, rootId)
  const scope = threadScope(channelId, rootId)
  const pending = usePendingSends(selectPendingSends(scope))
  const live = useLiveStreams(selectLiveStreams(scope))
  const progress = useRunProgress(selectRunProgress(scope))
  const send = useSendMessage(api, channelId, rootId)
  const cancel = useCancelRun(api)
  const agentNames = useAgentNames(api)
  const agents = useAgents(api)
  const nameOf = (row: TimelineRow): string | undefined =>
    row.authorAgentId === null ? undefined : agentNames[row.authorAgentId]

  const rows = useMemo(() => {
    // The merge expects the newest-first server order.
    const replies = [...(thread.data?.replies ?? [])].reverse()
    return mergeTimeline(replies, pending, live, progress)
  }, [thread.data, pending, live])

  const root = thread.data?.root
  const rootName =
    root === undefined
      ? undefined
      : root.author_kind === 'user'
        ? 'You'
        : ((root.author_agent_id == null ? undefined : agentNames[root.author_agent_id]) ?? 'Sprite')

  return (
    <aside className="thread-pane" data-testid="thread-pane">
      <div className="thread-header" data-testid="thread-pane-header">
        <span className="thread-title">Thread</span>
        {thread.data !== undefined && (
          <span className="thread-count">
            {rows.length === 1 ? '1 reply' : `${rows.length} replies`}
          </span>
        )}
        <Button variant="ghost" size="md" className="thread-desk" onClick={onOpenDesk}>
          <Monitor size={14} aria-hidden focusable="false" />
          Desk
        </Button>
        <IconButton
          icon={X}
          label="Close thread"
          variant="ghost"
          onClick={onClose}
        />
      </div>
      {thread.isPending ? (
        <div className="timeline-empty">Loading…</div>
      ) : root === undefined ? (
        <div className="timeline-empty">Thread not found.</div>
      ) : (
        <div className="thread-body">
          <div className="thread-root">
            <strong>{rootName}</strong>
            {' · '}
            {formatClock(root.created_at)}
            <p className="thread-root-text">{plainText(root.text_content)}</p>
          </div>
          <div className="thread-replies">
            {rows.map((row) =>
              // A live Run in the thread is the Working row.
              isProgressRow(row) && row.status === 'streaming' ? (
                <WorkingRow
                  key={row.key}
                  api={api}
                  row={row}
                  agentName={nameOf(row) ?? 'Sprite'}
                  agentAppearance={agents.data?.find((agent) => agent.id === row.authorAgentId)?.avatar}
                  onStop={(runId) => cancel.mutate(runId)}
                />
              ) : (
                <MessageRow
                  key={row.key}
                  api={api}
                  row={row}
                  authorAppearance={agents.data?.find((agent) => agent.id === row.authorAgentId)?.avatar}
                  authorName={nameOf(row)}
                  onRetry={(pendingId, text) => send.mutate({ pendingId, text })}
                />
              ),
            )}
          </div>
        </div>
      )}
      <ChannelComposer
        api={api}
        channelId={channelId}
        rootId={rootId}
        placeholder="Reply in thread"
      />
    </aside>
  )
}
