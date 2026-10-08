// The Memory place. The left
// column picks a scope, a view and a page; the right column shows the
// page as the daemon holds it, with its history and its sources, or
// the Changes view of the scope. The
// URL holds the scope, the page and the view, so the Agent profile can
// link a scope.

import { BookOpen } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useIsMobile } from '../../state/useIsMobile'

import type { AgentDto, ApiClient } from '../../api/client'
import {
  useAgents,
  useChannels,
  useConnections,
  useMemoryPageCounts,
  useMemoryPages,
  useWorkspace,
} from '../../queries'
import { PageState } from '../PageState'
import { chiefOfStaff } from '../sidebar/conversations'
import { ChangesView } from './ChangesView'
import { MemoryNav } from './MemoryNav'
import { PageRail } from './PageRail'
import { PageView } from './PageView'
import { parseScope, viewKind, type MemoryView } from './pages'

import './memory.css'

export interface MemoryLocation {
  scope: string
  path?: string
  view: MemoryView
}

export interface MemoryPageProps {
  api: ApiClient
  /** `agent:<id>` or `shared`; none opens on the Chief of Staff. */
  scope?: string
  /** The open page; none opens the newest page of the view. */
  path?: string
  view: MemoryView
  onChange: (next: MemoryLocation) => void
  onOpenChannel: (channelId: string) => void
  onOpenRun: (runId: string) => void
}

export function MemoryPage({
  api,
  scope,
  path,
  view,
  onChange,
  onOpenChannel,
  onOpenRun,
}: MemoryPageProps) {
  const roster = useAgents(api)
  const phone = useIsMobile()
  const workspace = useWorkspace(api)

  // The scope opens on the Chief of Staff, which the Workspace names,
  // so both reads must land before the page picks one (ADR-0022).
  if (scope === undefined && (roster.isPending || workspace.isPending)) {
    return <PageState icon={BookOpen} title="Loading memory" />
  }
  const agents = (roster.data ?? []).filter((agent) => agent.status === 'active')
  const chief = chiefOfStaff(agents, workspace.data?.chief_of_staff_agent_id)
  const resolved = scope ?? (phone || chief === null ? 'shared' : `agent:${chief.id}`)

  return (
    <ScopePage
      api={api}
      scope={resolved}
      path={path}
      view={view}
      agents={agents}
      chief={chief}
      onChange={onChange}
      onOpenChannel={onOpenChannel}
      onOpenRun={onOpenRun}
    />
  )
}

function ScopePage({
  api,
  scope,
  path,
  view,
  agents,
  chief,
  onChange,
  onOpenChannel,
  onOpenRun,
}: Omit<MemoryPageProps, 'scope'> & {
  scope: string
  agents: AgentDto[]
  chief: AgentDto | null
}) {
  const [search, setSearch] = useState('')
  const phone = useIsMobile()
  const needle = useSettled(search.trim(), SEARCH_DELAY_MS)
  // The list of the view opens its newest page. The search narrows
  // the rows only, so the open page stays while the user types.
  const kind = viewKind(view)
  const listed = useMemoryPages(api, scope, kind, '')
  const found = useMemoryPages(api, scope, kind, needle)
  const counts = useMemoryPageCounts(api, scope)
  const channels = useChannels(api)
  const connections = useConnections(api)
  const parsed = parseScope(scope)
  const owner =
    parsed?.kind === 'agent'
      ? (agents.find((agent) => agent.id === parsed.agentId) ?? null)
      : null

  const connectionName = (connectionId: string): string =>
    connections.data?.find((connection) => connection.id === connectionId)?.display_name ??
    connectionId

  const listedPages = listed.data?.pages.flatMap((part) => part.pages)
  const foundPages = found.data?.pages.flatMap((part) => part.pages)
  // A page that the view hides stays open when the URL names it. The
  // Changes view opens no page of its own.
  const openPath = view === 'changes' || phone ? path : (path ?? listedPages?.[0]?.path)

  return (
    <div className={phone ? 'memory memory-phone' : 'memory'} data-testid="memory">
      {(!phone || !path) && <MemoryNav
        scope={scope}
        owner={owner}
        agents={agents}
        view={view}
        pages={foundPages}
        matches={found.data?.pages[0]?.total}
        holds={counts.data?.pages}
        failed={found.isError}
        search={search}
        onSearch={setSearch}
        onShowMore={
          found.hasNextPage && !found.isFetchingNextPage
            ? () => void found.fetchNextPage()
            : undefined
        }
        openPath={openPath}
        connectionName={connectionName}
        onChange={onChange}
      />}
      {view === 'changes' ? (
        <ChangesView
          api={api}
          scope={scope}
          owner={owner}
          pages={listedPages ?? []}
          onOpenPage={(next) => onChange({ scope, path: next, view: 'pages' })}
          onOpenRun={onOpenRun}
        />
      ) : openPath === undefined ? (phone ? null :
        <section className="memory-page memory-page-empty">
          <PageState icon={BookOpen} title="No page is open">
            {listed.isPending ? 'Loading the pages.' : 'This scope holds no page yet.'}
          </PageState>
        </section>
      ) : (
        <section className="memory-page" key={`${scope} ${openPath}`}>
          <PageView
            api={api}
            scope={scope}
            path={openPath}
            owner={owner}
            asker={owner ?? chief}
            channels={channels.data ?? []}
            connectionName={connectionName}
            onOpenChannel={onOpenChannel}
          />
          <PageRail
            api={api}
            scope={scope}
            path={openPath}
            agents={agents}
            connectionName={connectionName}
          />
        </section>
      )}
    </div>
  )
}

/** The wait after the last key before the search asks the daemon. */
const SEARCH_DELAY_MS = 200

/** `value` after it has stayed the same for `delayMs`. */
function useSettled(value: string, delayMs: number): string {
  const [settled, setSettled] = useState(value)
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), delayMs)
    return () => clearTimeout(timer)
  }, [value, delayMs])
  return settled
}
