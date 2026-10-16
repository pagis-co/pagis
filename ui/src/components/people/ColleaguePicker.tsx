import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Button } from '../../primitives'
import { useColleagues, useStartConversation } from '../../queries'

function PresenceDot({ name, presence }: { name: string; presence: string }) {
  return (
    <span
      className={`person-presence person-presence-${presence}`}
      role="img"
      aria-label={`${name} is ${presence === 'in_a_meeting' ? 'in a meeting' : presence}`}
    />
  )
}

export function ColleaguePicker({
  api,
  onOpen,
}: {
  api: ApiClient
  onOpen: (id: string) => void
}) {
  const [selected, setSelected] = useState<string[]>([])
  const people = useColleagues(api)
  const start = useStartConversation(api)
  const toggle = (workspaceId: string) => {
    setSelected((ids) =>
      ids.includes(workspaceId)
        ? ids.filter((id) => id !== workspaceId)
        : [...ids, workspaceId],
    )
  }

  return (
    <>
      {people.isPending && <p>Loading colleagues…</p>}
      {people.isError && <p role="alert">Cannot load colleagues.</p>}
      {people.data?.length === 0 && <p>No colleagues.</p>}
      <fieldset>
        <legend>Choose colleagues</legend>
        {people.data?.map((person) => (
          <label className="people-colleague" key={person.workspace_id}>
            <input
              type="checkbox"
              checked={selected.includes(person.workspace_id)}
              onChange={() => toggle(person.workspace_id)}
            />
            <PresenceDot name={person.name} presence={person.presence} />
            {person.name}
            {person.presence === 'in_a_meeting' && <span>In a meeting</span>}
          </label>
        ))}
      </fieldset>
      <Button
        disabled={selected.length === 0 || start.isPending}
        onClick={() =>
          start.mutate(selected, {
            onSuccess: (conversation) => onOpen(conversation.id),
          })
        }
      >
        Start conversation
      </Button>
      {start.isError && <p role="alert">Cannot start the conversation.</p>}
    </>
  )
}
