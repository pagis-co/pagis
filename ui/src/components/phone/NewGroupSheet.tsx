import { useState } from 'react'
import { Info } from 'lucide-react'
import type { ApiClient } from '../../api/client'
import { Avatar, Frame, Input, Row, SectionLabel, Sheet } from '../../primitives'
import { errorMessage, useAgents, useCreateChannel } from '../../queries'

export function NewGroupSheet({
  api,
  open,
  onClose,
  onCreated,
}: {
  api: ApiClient
  open: boolean
  onClose: () => void
  onCreated: (id: string) => void
}) {
  const agents = useAgents(api)
  const create = useCreateChannel(api)
  const [title, setTitle] = useState('')
  const [selected, setSelected] = useState<string[]>([])
  const reset = () => {
    setTitle('')
    setSelected([])
    create.reset()
  }
  const close = () => {
    reset()
    onClose()
  }
  // The new group opens in place of the sheet: one navigation, with no
  // stop at the list on the way.
  const submit = () => {
    if (create.isPending || !title.trim() || !selected.length) return
    create.mutate(
      { title: title.trim(), agentIds: selected },
      {
        onSuccess: (channel) => {
          reset()
          onCreated(channel.id)
        },
      },
    )
  }
  return (
    <Sheet
      open={open}
      onOpenChange={(next) => {
        if (!next) close()
      }}
      title="New group"
      action={{
        label: 'Create',
        disabled: create.isPending || !title.trim() || !selected.length,
        onSelect: submit,
      }}
    >
      <form
        className="phone-form"
        onSubmit={(event) => {
          event.preventDefault()
          submit()
        }}
      >
        <div className="phone-form-field">
          <label htmlFor="group-name">Group name</label>
          <Input id="group-name" value={title} onChange={(event) => setTitle(event.target.value)} />
        </div>
        <div className="phone-section">
          <SectionLabel>Sprites in this group</SectionLabel>
          <Frame>
            {(agents.data ?? [])
              .filter((agent) => agent.status === 'active')
              .map((agent) => (
                <Row key={agent.id}>
                  <label className="phone-checkbox-row">
                    <Avatar id={agent.id} name={agent.name} appearance={agent.avatar} size="lg" />
                    <span className="phone-row-copy">
                      <strong>{agent.name}</strong>
                      <span className="phone-hint">{agent.job}</span>
                    </span>
                    <input
                      type="checkbox"
                      checked={selected.includes(agent.id)}
                      aria-label={agent.name}
                      onChange={(event) =>
                        setSelected((ids) =>
                          event.target.checked
                            ? [...ids, agent.id]
                            : ids.filter((id) => id !== agent.id),
                        )
                      }
                    />
                  </label>
                </Row>
              ))}
          </Frame>
          <p className="phone-hint">
            {selected.length} {selected.length === 1 ? 'sprite' : 'sprites'}. Later, use @mentions
            in the group to bring in another sprite.
          </p>
        </div>
        <p className="phone-well phone-hint phone-info-well">
          <Info size={20} aria-hidden />
          To talk to one sprite, open its conversation. Each sprite has one with you already.
        </p>
        {create.isError && (
          <p role="alert">{errorMessage(create.error, 'That group could not be made.')}</p>
        )}
      </form>
    </Sheet>
  )
}
