import { useState } from 'react'
import type { AgentDto, ApiClient } from '../api/client'
import { Button } from '../primitives'
import { errorMessage, useUpdateAppearance } from '../queries'
import { AppearanceControls } from './AppearanceControls'
import { SpriteAvatar } from './SpriteAvatar'
import { appearanceKey, defaultAppearance } from './catalog'
import type { SpriteClip } from './motion'

export function AgentAppearance({
  api,
  agent,
}: {
  api: ApiClient
  agent: AgentDto
}) {
  const [draft, setDraft] = useState(agent.avatar)
  const [saved, setSaved] = useState(agent.avatar)
  const [clip, setClip] = useState<SpriteClip>('Idle')
  const [take, setTake] = useState(0)
  const update = useUpdateAppearance(api)
  const dirty = appearanceKey(draft) !== appearanceKey(saved)
  return (
    <form
      className="agent-appearance"
      aria-label={`${agent.name} appearance`}
      onSubmit={(event) => {
        event.preventDefault()
        update.mutate(
          { agentId: agent.id, avatar: draft },
          {
            onSuccess: (agent) => {
              setSaved(agent.avatar)
              setDraft(agent.avatar)
            },
          },
        )
      }}
    >
      <div className="appearance-preview">
        <SpriteAvatar
          key={take}
          name={agent.name}
          appearance={draft}
          clip={clip}
          animate
          interactive
        />
        <p>Drag to turn. Choose a state to preview its motion.</p>
        <div className="appearance-actions">
          {(
            [
              ['Idle', 'Ready'],
              ['Working', 'Working'],
              ['NeedsInput', 'Needs you'],
              ['Celebrate', 'Celebrate'],
              ['Error', 'Oops'],
            ] as const
          ).map(([value, label]) => (
            <Button
              key={value}
              type="button"
              size="sm"
              aria-pressed={clip === value}
              onClick={() => {
                setClip(value)
                setTake((take) => take + 1)
              }}
            >
              {label}
            </Button>
          ))}
        </div>
      </div>
      <div>
        <fieldset
          disabled={update.isPending || agent.status === 'archived'}
          className="agent-appearance-fields"
        >
          <AppearanceControls
            value={draft}
            onChange={(draft) => {
              setDraft(draft)
              update.reset()
            }}
          />
          <div className="appearance-actions">
            <Button
              type="submit"
              variant="primary"
              disabled={!dirty || update.isPending}
            >
              Save appearance
            </Button>
            <Button
              type="button"
              disabled={!dirty}
              onClick={() => {
                setDraft(saved)
                update.reset()
              }}
            >
              Discard changes
            </Button>
            <Button
              type="button"
              onClick={() => {
                setDraft(defaultAppearance(draft.sprite))
                update.reset()
              }}
            >
              Reset style
            </Button>
          </div>
        </fieldset>
        <p
          className="appearance-status"
          role={update.isError ? 'alert' : 'status'}
        >
          {update.isError
            ? errorMessage(update.error, 'Appearance could not be saved.')
            : update.isPending
              ? 'Saving…'
              : update.isSuccess
                ? 'Appearance saved.'
                : dirty
                  ? 'You have unsaved changes.'
                  : ''}
        </p>
      </div>
    </form>
  )
}
