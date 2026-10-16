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
}: {
  api: ApiClient
  conversationId: string
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

  return (
    <section className="people-page workspace-conversation">
      <h1>Conversation</h1>
      {messages.isPending && <p>Loading messages…</p>}
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
          <MessageRow
            key={row.key}
            api={api}
            row={row}
            personId={person?.id}
            onRetry={(pendingId, text) => send.mutate({ pendingId, text })}
          />
        ))}
      </div>
      <ConversationComposer
        onSend={(text) => send.mutate({ pendingId: randomId(), text })}
      />
    </section>
  )
}
