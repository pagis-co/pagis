import { useEffect, useRef } from 'react'
import type { ApiClient } from '../../api/client'
import { randomId } from '../../ids'
import { Button } from '../../primitives'
import {
  useConversationMessages,
  useSendConversationMessage,
  useUser,
} from '../../queries'
import { selectPendingSends, usePendingSends } from '../../state/stores'
import { mergeTimeline } from '../../timeline'
import { MessageRow } from '../MessageRow'
import { ConversationComposer } from './ConversationComposer'
import '../People.css'

export function WorkspaceConversation({
  api,
  conversationId,
  targetMessageId,
}: {
  api: ApiClient
  conversationId: string
  targetMessageId?: string
}) {
  const messages = useConversationMessages(api, conversationId)
  const send = useSendConversationMessage(api, conversationId)
  const pending = usePendingSends(selectPendingSends(conversationId))
  const person = useUser(api).data
  const rows = mergeTimeline(
    messages.data?.pages.flatMap((page) => page.items) ?? [],
    pending,
    {},
    {},
    {
      kind: 'person',
      id: person?.id,
      name: person?.name ?? person?.email ?? 'Person',
    },
  )

  const target = useRef<HTMLDivElement>(null)
  const opened = useRef<string | undefined>(undefined)
  const found = rows.some(row => row.key === targetMessageId)
  useEffect(() => {
    if (targetMessageId === undefined || opened.current === targetMessageId) return
    if (found) {
      target.current?.scrollIntoView?.({ block: 'center' })
      target.current?.focus({ preventScroll: true })
      opened.current = targetMessageId
    } else if (messages.hasNextPage && !messages.isFetching && !messages.isError) {
      void messages.fetchNextPage()
    }
  }, [targetMessageId, found, messages.hasNextPage, messages.isFetching, messages.isError, messages.fetchNextPage])

  return (
    <section className="people-page workspace-conversation">
      <h1>Conversation</h1>
      {messages.isPending && <p>Loading messages…</p>}
      {targetMessageId && !found && !messages.isPending && !messages.hasNextPage && <p role="status">This message is unavailable.</p>}
      {messages.isError && <p role="alert">Cannot load messages.</p>}
      {messages.hasNextPage && (
        <Button
          onClick={() => void messages.fetchNextPage()}
          disabled={messages.isFetchingNextPage}
        >
          Older messages
        </Button>
      )}
      <div className="workspace-conversation-messages" aria-live="polite">
        {rows.map((row) => (
          <div key={row.key} ref={row.key === targetMessageId ? target : undefined}
            role={row.key === targetMessageId ? 'region' : undefined}
            aria-label={row.key === targetMessageId ? 'Selected message' : undefined}
            tabIndex={row.key === targetMessageId ? -1 : undefined}>
          <MessageRow
            api={api}
            row={row}
            personId={person?.id}
            onRetry={(pendingId, text) => send.mutate({ pendingId, text })}
          />
          </div>
        ))}
      </div>
      <ConversationComposer
        onSend={(text) => send.mutate({ pendingId: randomId(), text })}
      />
    </section>
  )
}
