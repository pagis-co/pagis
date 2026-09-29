// The rail of a memory page: the history with Revert per
// commit, then the sources of a private page or the readers of a
// shared one.

import type { AgentDto, ApiClient, MemoryFeedItem } from '../../api/client'
import { useMemoryFile, usePageHistory, useRevertCommit } from '../../queries'
import { Avatar, Badge, Button, Frame, Row, SectionLabel } from '../../primitives'
import { changeTimeLabel } from './pages'

export function PageRail({
  api,
  scope,
  path,
  agents,
  connectionName,
}: {
  api: ApiClient
  scope: string
  path: string
  agents: AgentDto[]
  connectionName: (connectionId: string) => string
}) {
  const history = usePageHistory(api, scope, path)
  const file = useMemoryFile(api, scope, path)
  const revert = useRevertCommit(api)
  const shared = scope === 'shared'
  // A Schedule entry is not a change of the file; the page shows it.
  const commits = (history.data?.items ?? []).filter((item) => item.kind !== 'schedule')
  const sources = file.data?.sources ?? []

  const onRevert = (item: MemoryFeedItem) => {
    const expectedRevision = history.data?.revision
    if (expectedRevision == null) return
    revert.mutate({ sha: item.sha, expectedRevision })
  }

  return (
    <aside className="memory-rail" aria-label="Page history">
      <SectionLabel id="memory-history-label">History of this page</SectionLabel>
      {history.isError && <p role="alert" className="memory-empty">Could not load the history.</p>}
      {history.data !== undefined && commits.length === 0 && (
        <p className="memory-empty">No change touched this page yet.</p>
      )}
      {commits.length > 0 && (
        <Frame role="list" aria-labelledby="memory-history-label">
          {commits.map((item) => (
            <Row key={item.id} className="memory-rail-row" role="listitem">
              <Avatar
                id={item.agent_id ?? 'you'}
                name={item.agent_name ?? 'You'}
                size="sm"
              />
              <span className="memory-rail-text">
                <span className="memory-rail-message">{item.message}</span>
                <span className="memory-rail-meta">
                  {changeTimeLabel(item.created_at)}
                  {item.files.length > 0 &&
                    ` · ${item.files.length} ${item.files.length === 1 ? 'file' : 'files'}`}
                </span>
              </span>
              {item.kind === 'reverted' ? (
                <Badge className="memory-rail-action">Reverted</Badge>
              ) : (
                <Button
                  variant="ghost"
                  size="sm"
                  className="memory-rail-action"
                  disabled={revert.isPending}
                  onClick={() => onRevert(item)}
                >
                  Revert
                </Button>
              )}
            </Row>
          ))}
        </Frame>
      )}
      {revert.isError && (
        <p role="alert" className="memory-empty">
          {revert.error.message}
        </p>
      )}

      {shared ? (
        <>
          <SectionLabel id="memory-readers-label">Who reads it</SectionLabel>
          <Frame
            hint={
              file.data?.source_scoped
                ? 'This page derives from a source. A sprite without a grant to that source cannot read it.'
                : undefined
            }
            role="list"
            aria-labelledby="memory-readers-label"
          >
            {agents.map((agent) => (
              <Row key={agent.id} className="memory-rail-row" role="listitem">
                <Avatar id={agent.id} name={agent.name} appearance={agent.avatar} size="sm" />
                <span className="memory-rail-message">{agent.name}</span>
                <Badge tone="accent" className="memory-rail-action">
                  reads and writes
                </Badge>
              </Row>
            ))}
          </Frame>
        </>
      ) : (
        <>
          <SectionLabel id="memory-sources-label">Sources</SectionLabel>
          {file.data !== undefined && sources.length === 0 && (
            <p className="memory-empty">This page comes from conversations.</p>
          )}
          {sources.length > 0 && (
            <Frame
              hint="Revert undoes one change on every file it touched. The source stays, and the next arrival can write the page again."
              role="list"
              aria-labelledby="memory-sources-label"
            >
              {sources.map((source) => (
                <Row
                  key={`${source.connection_id} ${source.resource}`}
                  className="memory-rail-row"
                  role="listitem"
                >
                  <span className="memory-source-tile" aria-hidden>
                    {(source.connection_id == null
                      ? source.resource ?? '?'
                      : connectionName(source.connection_id)
                    )
                      .charAt(0)
                      .toUpperCase()}
                  </span>
                  <span className="memory-rail-text">
                    <span className="memory-rail-message">
                      {source.connection_id == null
                        ? 'No connection'
                        : connectionName(source.connection_id)}
                    </span>
                    <span className="memory-rail-meta">
                      {[
                        `${source.count} ${source.count === 1 ? 'arrival' : 'arrivals'}`,
                        source.resource,
                      ]
                        .filter(Boolean)
                        .join(' · ')}
                    </span>
                  </span>
                </Row>
              ))}
            </Frame>
          )}
        </>
      )}
    </aside>
  )
}
