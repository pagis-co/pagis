import type { ApiClient } from '../../api/client'
import { useAgentNames, useColleagues, useUser } from '../../queries'
import type { CalendarEvent } from './queries'

function AgentAuthor({ api, id }: { api: ApiClient; id: string }) {
  const names = useAgentNames(api)
  return <p>Created by {names[id] ?? id}</p>
}

function PersonAuthor({ api, id }: { api: ApiClient; id: string }) {
  const user = useUser(api)
  const colleagues = useColleagues(api)
  const name = user.data?.id === id
    ? user.data.name
    : colleagues.data?.find((person) => person.person_id === id)?.name
  return <p>Created by {name ?? id}</p>
}

export function EventAuthor({ api, author }: { api: ApiClient; author: CalendarEvent['created_by'] }) {
  switch (author.kind) {
    case 'agent': return <AgentAuthor api={api} id={author.agent_id} />
    case 'person': return <PersonAuthor api={api} id={author.person_id} />
    case 'system': return <p>Created by System</p>
  }
}
