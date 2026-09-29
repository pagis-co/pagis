// The new-group form: a name plus the Agent participants.

import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Button, Input } from '../../primitives'
import { useAgents, useCreateChannel } from '../../queries'

export function NewGroupForm({
  api,
  onCreated,
  onCancel,
}: {
  api: ApiClient
  onCreated: (channelId: string) => void
  onCancel: () => void
}) {
  const agents = useAgents(api)
  const create = useCreateChannel(api)
  const [title, setTitle] = useState('')
  const [selected, setSelected] = useState<string[]>([])

  const active = (agents.data ?? []).filter((agent) => agent.status === 'active')
  const toggle = (agentId: string) => {
    setSelected((ids) =>
      ids.includes(agentId) ? ids.filter((id) => id !== agentId) : [...ids, agentId],
    )
  }

  return (
    <form
      className="new-group-form"
      data-testid="new-group-form"
      onSubmit={(event) => {
        event.preventDefault()
        create.mutate(
          { title: title.trim(), agentIds: selected },
          { onSuccess: (channel) => onCreated(channel.id) },
        )
      }}
    >
      <Input
        aria-label="Group name"
        placeholder="Group name"
        value={title}
        onChange={(event) => setTitle(event.target.value)}
      />
      {active.map((agent) => (
        <label key={agent.id} className="new-group-agent">
          <input
            type="checkbox"
            checked={selected.includes(agent.id)}
            onChange={() => toggle(agent.id)}
          />
          {agent.name}
        </label>
      ))}
      <div className="new-group-actions">
        <Button
          type="submit"
          variant="primary"
          disabled={create.isPending || title.trim() === ''}
        >
          Create
        </Button>
        <Button type="button" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  )
}
