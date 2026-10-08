// The Memory tab of an Agent profile. The
// tab is a summary: what the Agent holds, what it learns from and its
// last three changes. The pages and the full history live in the
// Memory place, so every count links there.

import { useQueryClient } from '@tanstack/react-query'
import { ChevronRight, MessageSquare, Phone } from 'lucide-react'

import type { AgentDto, ApiClient, MemoryFeedItem } from '../../api/client'
import { formatE164 } from '../../blocks/call'
import { Avatar, Badge, Button, Frame, Row, SectionLabel } from '../../primitives'
import {
  useAgentSyncConnections,
  useAgents,
  useMemoryPageCounts,
  usePhoneNumbers,
  useRecentChanges,
  useRevertCommit,
} from '../../queries'
import type { MemoryLocation } from '../memory/MemoryPage'
import { changeTimeLabel } from '../memory/pages'

import { useIsMobile } from '../../state/useIsMobile'
import './sprites.css'

/** The number of changes the tab shows. */
const RECENT_CHANGES = 3

function counted(count: number, singular: string, plural: string): string {
  return `${count} ${count === 1 ? singular : plural}`
}

/** A list of names as one phrase: `A`, `A and B`, `A, B and C`. */
function namesPhrase(names: string[]): string {
  if (names.length <= 1) return names.join('')
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

/** The scope-relative path of a feed file (`private/…` or `shared/…`). */
function relativeFile(file: string): string {
  return file.replace(/^(private|shared)\//, '')
}

export interface AgentMemoryProps {
  api: ApiClient
  agent: AgentDto
  onOpenMemory: (location: Omit<MemoryLocation, 'path'>) => void
  onOpenSyncSettings: () => void
  onOpenRun: (runId: string) => void
}

export function AgentMemory({
  api,
  agent,
  onOpenMemory,
  onOpenSyncSettings,
  onOpenRun,
}: AgentMemoryProps) {
  const scope = `agent:${agent.id}`
  return (
    <section className="agent-memory" aria-label={`${agent.name} memory`}>
      <div className="agent-memory-columns">
        <Holds api={api} agent={agent} scope={scope} onOpenMemory={onOpenMemory} />
        <LearnsFrom api={api} agent={agent} onOpenSyncSettings={onOpenSyncSettings} />
      </div>
      <RecentChanges
        api={api}
        agent={agent}
        scope={scope}
        onOpenMemory={onOpenMemory}
        onOpenRun={onOpenRun}
      />
      <p className="settings-hint agent-memory-desktop-hint">
        This tab is the summary. The pages and the full history live in Memory, opened on{' '}
        {agent.name}.
      </p>
    </section>
  )
}

function Holds({
  api,
  agent,
  scope,
  onOpenMemory,
}: {
  api: ApiClient
  agent: AgentDto
  scope: string
  onOpenMemory: AgentMemoryProps['onOpenMemory']
}) {
  const phone = useIsMobile()
  const own = useMemoryPageCounts(api, scope)
  const shared = useMemoryPageCounts(api, 'shared')
  const roster = useAgents(api)
  const procedures = own.data?.procedures ?? 0
  const privatePages = (own.data?.pages ?? 0) - procedures
  const wrote =
    shared.data?.authors.find((author) => author.agent_id === agent.id)?.pages ?? 0
  const others = (roster.data ?? [])
    .filter((other) => other.status === 'active' && other.id !== agent.id)
    .map((other) => other.name)
  const failed = own.isError || shared.isError

  return (
    <div className="agent-memory-group">
      <SectionLabel id="agent-memory-holds">Holds</SectionLabel>
      {failed && (
        <p role="alert" className="settings-hint">
          Could not load the pages.
        </p>
      )}
      <Frame role="list" aria-labelledby="agent-memory-holds">
        <Row role="listitem" chevron={phone} onClick={phone ? () => onOpenMemory({ scope, view: 'pages' }) : undefined}>
          <span className="agent-memory-text">
            <span>
              {own.data === undefined
                ? 'Private pages'
                : counted(privatePages, 'private page', 'private pages')}
            </span>
            <span className="agent-memory-meta">
              people, companies, matters; only {agent.name} reads them
            </span>
          </span>
          {!phone && <Button
            variant="ghost"
            size="sm"
            className="agent-memory-action"
            onClick={() => onOpenMemory({ scope, view: 'pages' })}
          >
            Open in Memory
          </Button>}
        </Row>
        <Row role="listitem" chevron={phone} onClick={phone ? () => onOpenMemory({ scope, view: 'procedures' }) : undefined}>
          <span className="agent-memory-text">
            <span>
              {own.data === undefined
                ? 'Procedures'
                : counted(procedures, 'procedure', 'procedures')}
            </span>
            <span className="agent-memory-meta">how {agent.name} does its work</span>
          </span>
          {!phone && <Button
            variant="ghost"
            size="sm"
            className="agent-memory-action"
            onClick={() => onOpenMemory({ scope, view: 'procedures' })}
          >
            See
          </Button>}
        </Row>
        <Row role="listitem" chevron={phone} onClick={phone ? () => onOpenMemory({ scope: 'shared', view: 'pages' }) : undefined}>
          <span className="agent-memory-text">
            <span>
              {shared.data === undefined
                ? 'Shared pages'
                : counted(shared.data.pages, 'shared page', 'shared pages')}
            </span>
            <span className="agent-memory-meta">
              {others.length === 0 ? '' : `with ${namesPhrase(others)}; `}
              {agent.name} wrote {wrote} of them
            </span>
          </span>
          {!phone && <Button
            variant="ghost"
            size="sm"
            className="agent-memory-action"
            onClick={() => onOpenMemory({ scope: 'shared', view: 'pages' })}
          >
            Open shared
          </Button>}
        </Row>
      </Frame>
    </div>
  )
}

function LearnsFrom({
  api,
  agent,
  onOpenSyncSettings,
}: {
  api: ApiClient
  agent: AgentDto
  onOpenSyncSettings: () => void
}) {
  const phone = useIsMobile()
  const connections = useAgentSyncConnections(api, agent.id)
  const numbers = usePhoneNumbers(api)
  const deskLine = (numbers.data?.items ?? []).find((number) => number.agent_id === agent.id)

  return (
    <div className="agent-memory-group">
      <SectionLabel id="agent-memory-learns">Learns from</SectionLabel>
      {connections.isError && (
        <p role="alert" className="settings-hint">
          Could not load the connections.
        </p>
      )}
      <Frame role="list" aria-labelledby="agent-memory-learns">
        {(connections.data ?? []).map((connection) => (
          <Row key={connection.connection_id} role="listitem" onClick={phone ? onOpenSyncSettings : undefined}>
            <span className="agent-memory-tile" aria-hidden>
              {connection.display_name.slice(0, 1).toUpperCase()}
            </span>
            <span className="agent-memory-text">
              <span>{connection.display_name}</span>
              <span className="agent-memory-meta">
                Responsible sprite ·{' '}
                {!connection.enabled
                  ? 'paused'
                  : connection.caught_up
                    ? 'up to date'
                    : 'catching up'}{' '}
                · {counted(connection.rule_count, 'rule', 'rules')}
              </span>
            </span>
            {!phone && <Button
              variant="ghost"
              size="sm"
              className="agent-memory-action"
              onClick={onOpenSyncSettings}
            >
              Sync settings
            </Button>}
          </Row>
        ))}
        {deskLine !== undefined && (
          <Row role="listitem">
            <span className="agent-memory-tile" aria-hidden>
              <Phone size={14} />
            </span>
            <span className="agent-memory-text">
              <span>Desk line {formatE164(deskLine.e164)}</span>
              <span className="agent-memory-meta">calls it answers</span>
            </span>
          </Row>
        )}
        <Row role="listitem">
          <span className="agent-memory-tile" aria-hidden>
            <MessageSquare size={14} />
          </span>
          <span className="agent-memory-text">
            <span>Its conversations</span>
            <span className="agent-memory-meta">every completed Run reflects</span>
          </span>
        </Row>
      </Frame>
    </div>
  )
}

function RecentChanges({
  api,
  agent,
  scope,
  onOpenMemory,
  onOpenRun,
}: {
  api: ApiClient
  agent: AgentDto
  scope: string
  onOpenMemory: AgentMemoryProps['onOpenMemory']
  onOpenRun: (runId: string) => void
}) {
  const phone = useIsMobile()
  const changes = useRecentChanges(api, scope, RECENT_CHANGES)
  const revert = useRevertCommit(api)
  const queryClient = useQueryClient()
  const items = changes.data?.items ?? []

  const onRevert = (item: MemoryFeedItem) => {
    const expectedRevision = changes.data?.revision
    if (expectedRevision == null) return
    revert.mutate(
      { sha: item.sha, expectedRevision },
      {
        onSuccess: () => {
          void queryClient.invalidateQueries({ queryKey: ['memory-feed'] })
          void queryClient.invalidateQueries({ queryKey: ['memory-pages'] })
          void queryClient.resetQueries({ queryKey: ['memory-file'] })
        },
      },
    )
  }

  return (
    <>
      <div className="agent-memory-heading">
        <SectionLabel id="agent-memory-changes">Recent changes</SectionLabel>
        <Button
          variant="link"
          size="sm"
          className={`agent-memory-action${phone ? ' phone-accent' : ''}`}
          onClick={() => onOpenMemory({ scope, view: 'changes' })}
        >
          {phone ? 'See all' : `All of ${agent.name}’s changes in Memory`}
          <ChevronRight size={14} aria-hidden />
        </Button>
      </div>
      {changes.isError && (
        <p role="alert" className="settings-hint">
          Could not load the changes.
        </p>
      )}
      {changes.data !== undefined && items.length === 0 && (
        <p className="settings-hint">{agent.name} has changed no memory yet.</p>
      )}
      {items.length > 0 && (
        <Frame role="list" aria-labelledby="agent-memory-changes">
          {items.map((item) => (
            <Row key={item.id} role="listitem">
              <Avatar
                id={item.agent_id ?? 'you'}
                name={item.agent_name ?? 'You'}
                appearance={item.agent_id === agent.id ? agent.avatar : undefined}
                size="sm"
              />
              <span className="agent-memory-text">
                <span>
                  <strong>{item.agent_name ?? 'You'}</strong> <span>{item.message}</span>
                </span>
                <span className="agent-memory-meta agent-memory-files">
                  {changeTimeLabel(item.created_at)}
                  {item.files.map((file) => (
                    <Badge key={file}>{relativeFile(file)}</Badge>
                  ))}
                </span>
              </span>
              {!phone && <span className="agent-memory-action agent-memory-buttons">
                {item.run_id != null && (
                  <Button variant="ghost" size="sm" onClick={() => onOpenRun(item.run_id!)}>
                    Open the Run
                  </Button>
                )}
                {item.kind === 'reverted' ? (
                  <Badge>Reverted</Badge>
                ) : (
                  item.kind === 'committed' && (
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={revert.isPending}
                      onClick={() => onRevert(item)}
                    >
                      Revert
                    </Button>
                  )
                )}
              </span>}
            </Row>
          ))}
        </Frame>
      )}
      {revert.isError && (
        <p role="alert" className="settings-hint">
          {revert.error.message}
        </p>
      )}
    </>
  )
}
