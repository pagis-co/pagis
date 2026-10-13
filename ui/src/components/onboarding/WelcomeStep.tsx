// Welcome introduces sprites and names the Chief of Staff.

import { useState } from 'react'
import { History, Monitor, NotebookText } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'

import type { AgentDto, ApiClient } from '../../api/client'
import { errorMessage, useUpdateAgent } from '../../queries'
import { Avatar, Button, Input } from '../../primitives'

import { StepLayout } from './StepLayout'

interface Capability {
  icon: LucideIcon
  name: string
  line: string
}

const CAPABILITIES: Capability[] = [
  {
    icon: Monitor,
    name: 'Computer',
    line: 'Each sprite has its own computer to browse, run tools, and get things done.',
  },
  {
    icon: NotebookText,
    name: 'Memory',
    line: 'Sprites remember useful context from your work.',
  },
  {
    icon: History,
    name: 'Follow-ups',
    line: 'Sprites check back, continue tasks, and keep things moving.',
  },
]

export function WelcomeStep({
  api,
  sprite,
  onContinue,
}: {
  api: ApiClient
  sprite: AgentDto | undefined
  onContinue: () => void
}) {
  const [name, setName] = useState<string | null>(null)
  const update = useUpdateAgent(api)
  const spriteName = name ?? sprite?.name ?? ''
  const next = async () => {
    if (sprite !== undefined && spriteName.trim() !== sprite.name) {
      try {
        await update.mutateAsync({
          agentId: sprite.id,
          name: spriteName.trim(),
          job: sprite.job,
          description: sprite.description,
          personality: sprite.personality,
          voice: sprite.voice ?? null,
          standing_brief: sprite.standing_brief ?? null,
        })
      } catch {
        return
      }
    }
    onContinue()
  }
  return (
    <StepLayout
      action={
        <Button
          variant="primary"
          disabled={update.isPending || sprite === undefined || !spriteName.trim()}
          onClick={() => void next()}
        >
          Continue
        </Button>
      }
    >
      <header className="onboarding-head">
        <h1>Welcome to Pagis</h1>
        <p className="onboarding-lead">
          Sprites are AI agents that work as your virtual assistants. They use
          their own computers, remember useful context, and follow up on tasks.
        </p>
      </header>

      <ul className="onboarding-capabilities">
        {CAPABILITIES.map((capability) => {
          const Icon = capability.icon
          return (
            <li key={capability.name}>
              <Icon size={20} aria-hidden />
              <p className="onboarding-capability-name">{capability.name}</p>
              <p className="onboarding-capability-line">{capability.line}</p>
            </li>
          )
        })}
      </ul>

      {sprite === undefined ? null : (
        <div className="onboarding-sprite">
          <Avatar
            id={sprite.id}
            name={sprite.name}
            appearance={sprite.avatar}
            size="lg"
          />
          <div>
            <label className="onboarding-field">
              Sprite name
              <Input
                aria-label="Sprite name"
                value={spriteName}
                onChange={(event) => setName(event.target.value)}
              />
            </label>
            <p className="onboarding-sprite-line">
              Your first and main sprite, the Chief of Staff. Rename it whenever you like.
            </p>
          </div>
        </div>
      )}
      {update.isError && (
        <p className="onboarding-error" role="alert">
          {errorMessage(update.error, 'Cannot save the sprite name.')}
        </p>
      )}
    </StepLayout>
  )
}
