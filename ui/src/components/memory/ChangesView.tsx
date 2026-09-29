// The Changes view of one scope: every
// commit and revert that touched the scope, one row each, grouped by
// day and narrowed by where the change came from.

import { useState } from 'react'

import type { AgentDto, ApiClient } from '../../api/client'
import { Button, Frame, SectionLabel } from '../../primitives'
import { useMemoryChanges } from '../../queries'
import { ChangeRow } from './ChangeRow'
import { CHANGE_FILTERS, groupChanges, revertedShas, type ChangeKind } from './changes'
import type { MemoryPageDto } from './pages'

export function ChangesView({
  api,
  scope,
  owner,
  pages,
  onOpenPage,
  onOpenRun,
}: {
  api: ApiClient
  scope: string
  /** The Agent of a private scope; `null` for Shared. */
  owner: Pick<AgentDto, 'name'> | null
  /** The pages of the scope, for the chip titles. */
  pages: readonly MemoryPageDto[]
  onOpenPage: (path: string) => void
  onOpenRun: (runId: string) => void
}) {
  const [kind, setKind] = useState<ChangeKind | undefined>(undefined)
  const changes = useMemoryChanges(api, scope, kind)
  // A filter can hide the revert of a commit it shows, so the reverts
  // of the scope are read on their own.
  const reverts = useMemoryChanges(api, scope, 'revert')
  const reverted = revertedShas(reverts.data?.items ?? [])
  const groups = groupChanges(changes.data?.items ?? [])
  const title = owner === null ? 'Changes to shared memory' : `Changes to ${owner.name}’s memory`

  const pageTitle = (path: string): string =>
    pages.find((page) => page.path === path)?.title ?? path

  return (
    <section className="memory-changes" aria-label="Changes">
      <header className="memory-changes-header">
        <h1>{title}</h1>
        <span className="memory-note">one revertible commit each</span>
        <div role="group" aria-label="Filters" className="memory-changes-filters">
          {CHANGE_FILTERS.map((filter) => (
            <Button
              key={filter.label}
              variant="ghost"
              size="sm"
              shape="pill"
              className="memory-view"
              aria-pressed={filter.kind === kind}
              onClick={() => setKind(filter.kind)}
            >
              {filter.label}
            </Button>
          ))}
        </div>
      </header>

      {changes.isError && (
        <p role="alert" className="memory-empty">
          Could not load the changes.
        </p>
      )}
      {changes.data !== undefined && groups.length === 0 && (
        <p className="memory-empty">No change matches this filter.</p>
      )}
      {groups.map((group) => {
        const labelId = `memory-changes-${group.label.replace(/\W+/g, '-')}`
        return (
          <div key={group.label} className="memory-changes-group">
            <SectionLabel id={labelId}>{group.label}</SectionLabel>
            <Frame role="list" aria-labelledby={labelId}>
              {group.items.map((item) => (
                <ChangeRow
                  key={item.id}
                  api={api}
                  scope={scope}
                  item={item}
                  reverted={item.kind === 'reverted' || reverted.has(item.sha)}
                  revision={changes.data?.revision ?? null}
                  pageTitle={pageTitle}
                  onOpenPage={onOpenPage}
                  onOpenRun={onOpenRun}
                />
              ))}
            </Frame>
          </div>
        )
      })}
    </section>
  )
}
