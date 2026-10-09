// `/coding`: the Coding place, every Coding Session of the Workspace
// (ADR-0022, ADR-0033).
//
// "Open" holds each session that is not settled: the ones whose
// decision waits for the Person first, then the last activity, newest
// first. "Ended" holds the settled ones, newest end first. The "Needs
// you" mark reads the pending decision of the record; it is a mark on
// the record and not a second queue. A frame of any session makes the
// list read again (`AppShell`).

import { SquareTerminal } from 'lucide-react'
import { useMemo } from 'react'

import type { AgentDto, ApiClient, CodingSessionDto } from '../../api/client'
import { Avatar, Badge, Button, Frame } from '../../primitives'
import { useAgents, useChannels, useCodingSessions, useWorkspace } from '../../queries'
import { formatMoment } from '../../timeline'
import { directMessageChannel } from '../AskAnAgent'
import { PageState } from '../PageState'
import { chiefOfStaff } from '../sidebar/conversations'
import { sessionSettled, sessionStateBadge } from './words'

import './coding.css'

/** A session whose pending decision waits for the Person. */
function needsYou(session: CodingSessionDto): boolean {
  return session.pending?.waits_for === 'person'
}

/** The open sessions and the ended ones, each in its reading order. */
function sections(sessions: readonly CodingSessionDto[]) {
  const open = sessions
    .filter((session) => !sessionSettled(session.state))
    .sort(
      (a, b) =>
        Number(needsYou(b)) - Number(needsYou(a)) || b.updated_at - a.updated_at,
    )
  const ended = sessions
    .filter((session) => sessionSettled(session.state))
    .sort((a, b) => (b.ended_at ?? b.updated_at) - (a.ended_at ?? a.updated_at))
  return { open, ended }
}

function SessionRow({
  session,
  agent,
  onOpen,
}: {
  session: CodingSessionDto
  agent: AgentDto | undefined
  onOpen: () => void
}) {
  const state = sessionStateBadge(session.state)
  const spriteName = agent?.name ?? 'A sprite'
  // A session in the Agent's Computer has no Host name.
  const machine = session.machine_name ?? 'Computer'
  return (
    <Button shape="row" variant="ghost" className="coding-list-row" onClick={onOpen}>
      <Avatar appearance={agent?.avatar} id={session.agent_id} name={spriteName} size="sm" />
      <span className="coding-list-what">
        <strong className="coding-list-title">{session.title}</strong>
        <span className="coding-list-facts">
          <span>{spriteName}</span>
          <span aria-hidden> · </span>
          <span>{session.harness_name}</span>
          <span aria-hidden> · </span>
          <span>{machine}</span>
          <span aria-hidden> · </span>
          <span className="coding-list-directory" title={session.directory}>
            {session.directory}
          </span>
        </span>
      </span>
      <span className="coding-list-badges">
        {needsYou(session) && <Badge tone="waiting">Needs you</Badge>}
        <Badge tone={state.tone}>{state.label}</Badge>
      </span>
      <span className="coding-list-time">{formatMoment(session.updated_at)}</span>
    </Button>
  )
}

function Section({
  title,
  sessions,
  agents,
  onOpenSession,
}: {
  title: string
  sessions: readonly CodingSessionDto[]
  agents: readonly AgentDto[]
  onOpenSession: (sessionId: string) => void
}) {
  if (sessions.length === 0) return null
  return (
    <section className="coding-list-section" aria-label={title}>
      <h3>{title}</h3>
      <Frame>
        {sessions.map((session) => (
          <SessionRow
            key={session.id}
            session={session}
            agent={agents.find((agent) => agent.id === session.agent_id)}
            onOpen={() => onOpenSession(session.id)}
          />
        ))}
      </Frame>
    </section>
  )
}

export function CodingSessions({
  api,
  onOpenSession,
  onOpenChannel,
}: {
  api: ApiClient
  onOpenSession: (sessionId: string) => void
  onOpenChannel: (channelId: string) => void
}) {
  const list = useCodingSessions(api)
  const agents = useAgents(api)
  const channels = useChannels(api)
  const workspace = useWorkspace(api)

  const all = useMemo(
    () => (list.data?.pages ?? []).flatMap((page) => page.items),
    [list.data],
  )
  const { open, ended } = useMemo(() => sections(all), [all])

  const chief = chiefOfStaff(agents.data ?? [], workspace.data?.chief_of_staff_agent_id)
  const chiefChannelId =
    chief === null ? null : directMessageChannel(channels.data ?? [], chief.id)

  let body
  if (list.isError) {
    body = (
      <PageState
        icon={SquareTerminal}
        title="The coding sessions did not load."
        onRetry={() => void list.refetch()}
      />
    )
  } else if (list.data === undefined) {
    body = <p className="coding-empty">Reading the coding sessions…</p>
  } else if (all.length === 0) {
    body = (
      <div className="coding-list-empty">
        <p className="coding-empty">
          No coding session yet. A sprite starts one when you ask it to change code.
        </p>
        {chief !== null && chiefChannelId !== null && (
          <Button onClick={() => onOpenChannel(chiefChannelId)}>Ask {chief.name}</Button>
        )}
      </div>
    )
  } else {
    body = (
      <>
        <Section
          title="Open"
          sessions={open}
          agents={agents.data ?? []}
          onOpenSession={onOpenSession}
        />
        <Section
          title="Ended"
          sessions={ended}
          agents={agents.data ?? []}
          onOpenSession={onOpenSession}
        />
      </>
    )
  }

  return (
    <div className="coding-page">
      <header className="coding-list-header">
        <h2>Coding</h2>
      </header>
      <p className="coding-list-summary">Every coding session of your sprites.</p>
      {body}
    </div>
  )
}
