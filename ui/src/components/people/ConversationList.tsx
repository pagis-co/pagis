import type { ApiClient } from '../../api/client'
import { Button } from '../../primitives'
import { useConversations } from '../../queries'

export function ConversationList({
  api,
  onOpen,
}: {
  api: ApiClient
  onOpen: (id: string) => void
}) {
  const conversations = useConversations(api)

  return (
    <>
      <h2>Conversations</h2>
      {conversations.isPending && <p>Loading conversations…</p>}
      {conversations.isError && <p role="alert">Cannot load conversations.</p>}
      {conversations.data?.map((conversation) => (
        <div key={conversation.id}>
          <Button onClick={() => onOpen(conversation.id)}>
            {conversation.participants.map((person) => person.name).join(', ')}
          </Button>
          {conversation.last_message && (
            <p>
              {conversation.last_message.author.name}:{' '}
              {conversation.last_message.text_content}
            </p>
          )}
        </div>
      ))}
    </>
  )
}
