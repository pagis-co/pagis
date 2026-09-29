// Step one: what a sprite is, in three lines and three
// pictures, and the sprite the workspace already has. Nothing here is
// recorded; it is the one page that explains the product.

import { History, Monitor, NotebookText } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'

import type { AgentDto } from '../../api/client'
import { Avatar, Button } from '../../primitives'

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
  sprite,
  onContinue,
}: {
  sprite: AgentDto | undefined
  onContinue: () => void
}) {
  return (
    <StepLayout
      action={
        <Button variant="primary" onClick={onContinue}>
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
            <p className="onboarding-sprite-name">{sprite.name}</p>
            <p className="onboarding-sprite-line">
              Your first and main sprite. Rename it whenever you like.
            </p>
          </div>
        </div>
      )}
    </StepLayout>
  )
}
