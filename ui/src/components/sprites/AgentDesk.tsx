// The Desk section of an Agent profile: the computer this Agent
// works at. The computer tile carries the live
// screen, Wake, Take over and Hand back; the Desk adds
// the words about the disk and about a missing Docker (ADR-0024).

import type { AgentDto, ApiClient } from '../../api/client'
import { ComputerTile, NoDocker } from '../Computers'
import { useOnboarding } from '../../queries'

import './sprites.css'

export function AgentDesk({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const onboarding = useOnboarding(api)
  const dockerMissing = onboarding.data !== undefined && onboarding.data.docker.endpoint == null

  return (
    <section className="agent-desk" aria-label={`${agent.name} desk`}>
      <p className="settings-hint">
        {agent.name} has a computer of its own. It wakes on demand and stops
        again when idle.
      </p>
      {dockerMissing && <NoDocker />}
      <ComputerTile api={api} agent={agent} />
      <p className="settings-hint" data-testid="agent-desk-disk">
        The disk keeps its files between sessions. A stop loses nothing.
      </p>
    </section>
  )
}
