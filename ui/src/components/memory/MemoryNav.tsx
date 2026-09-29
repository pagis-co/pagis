// The left column of the Memory place: the scope switcher, the
// views, the search and the page list grouped by change time.

import { Menu as MenuIcon, MessageSquare } from 'lucide-react'
import type { AgentDto } from '../../api/client'
import { Avatar, Button, IconButton, Input, SectionLabel, cx } from '../../primitives'
import type { MemoryLocation } from './MemoryPage'
import { changeTimeLabel, groupPages, type MemoryPageDto, type MemoryView } from './pages'

const VIEWS: { value: MemoryView; label: string }[] = [
  { value: 'pages', label: 'Pages' },
  { value: 'changes', label: 'Changes' },
  { value: 'procedures', label: 'Procedures' },
]

export function MemoryNav({
  scope,
  owner,
  agents,
  view,
  pages,
  matches,
  holds,
  failed,
  search,
  onSearch,
  onShowMore,
  openPath,
  connectionName,
  onChange,
  onOpenNav,
}: {
  scope: string
  /** The Agent of a private scope; `null` for Shared. */
  owner: AgentDto | null
  agents: AgentDto[]
  view: MemoryView
  /** The rows the daemon has given for the view and the search. */
  pages: MemoryPageDto[] | undefined
  /** How many pages match the view and the search, in all parts. */
  matches: number | undefined
  /** How many pages the scope holds. */
  holds: number | undefined
  failed: boolean
  search: string
  onSearch: (search: string) => void
  /** Ask for the next part; absent at the end of the list. */
  onShowMore: (() => void) | undefined
  openPath: string | undefined
  connectionName: (connectionId: string) => string
  onChange: (next: MemoryLocation) => void
  /** Open the sidebar drawer (a phone only). */
  onOpenNav: () => void
}) {
  const searchLabel = owner === null ? 'Search shared pages' : `Search ${owner.name}’s pages`
  const noun = holds === 1 ? 'page' : 'pages'
  const more = (matches ?? 0) - (pages?.length ?? 0)
  // A row opens its page, so the Changes view gives way to Pages.
  const pageView = view === 'changes' ? 'pages' : view

  return (
    <nav aria-label="Memory" className="memory-nav">
      <div className="memory-title">
        <IconButton
          icon={MenuIcon}
          label="Open conversations"
          variant="ghost"
          className="mobile-navigation-trigger"
          onClick={onOpenNav}
        />
        <h2>Memory</h2>
      </div>

      <div role="group" aria-label="Scope" className="memory-scopes">
        {[...agents.map((agent) => ({ value: `agent:${agent.id}`, agent })), {
          value: 'shared',
          agent: null,
        }].map(({ value, agent }) => (
          <Button
            key={value}
            variant="ghost"
            size="sm"
            className={cx('memory-scope', value === scope && 'memory-scope-current')}
            aria-pressed={value === scope}
            onClick={() => {
              onSearch('')
              onChange({ scope: value, view })
            }}
          >
            {agent !== null && (
              <Avatar id={agent.id} name={agent.name} appearance={agent.avatar} size="sm" />
            )}
            {agent?.name ?? 'Shared'}
          </Button>
        ))}
      </div>

      <div role="group" aria-label="Views" className="memory-views">
        {VIEWS.map((option) => (
          <Button
            key={option.value}
            variant="ghost"
            size="sm"
            shape="pill"
            className="memory-view"
            aria-pressed={option.value === view}
            onClick={() =>
              onChange({
                scope,
                view: option.value,
                ...(openPath === undefined ? {} : { path: openPath }),
              })
            }
          >
            {option.label}
          </Button>
        ))}
      </div>

      <Input
        aria-label={searchLabel}
        placeholder={searchLabel}
        className="memory-search"
        value={search}
        onChange={(event) => onSearch(event.target.value)}
      />
      {failed && (
        <p role="alert" className="memory-empty">
          Could not load the pages.
        </p>
      )}
      {pages?.length === 0 && search.trim() !== '' && (
        <p className="memory-empty">No page matches the search.</p>
      )}
      {groupPages(pages ?? []).map((group) => (
        <div key={group.label} className="memory-group">
          <SectionLabel className="memory-group-label">{group.label}</SectionLabel>
          {group.pages.map((page) => (
            <Button
              key={page.path}
              variant="ghost"
              className={cx('memory-row', page.path === openPath && 'memory-row-current')}
              aria-current={page.path === openPath ? 'page' : undefined}
              onClick={() => onChange({ scope, path: page.path, view: pageView })}
            >
              <span className="memory-row-text">
                <span className="memory-row-title">{page.title}</span>
                <span className="memory-row-meta">
                  {[page.kind, changeTimeLabel(page.changed_at)]
                    .filter((part) => part != null)
                    .join(' · ')}
                </span>
              </span>
              <SourceMark page={page} connectionName={connectionName} />
            </Button>
          ))}
        </div>
      ))}
      {onShowMore !== undefined && (
        <Button variant="ghost" size="sm" className="memory-more" onClick={onShowMore}>
          Show more pages ({more} more)
        </Button>
      )}
      {holds !== undefined && (
        <p className="memory-nav-note">
          {owner === null
            ? `Shared holds ${holds} ${noun}. Every sprite reads them, unless a page derives from a source that the sprite cannot read.`
            : `${owner.name} holds ${holds} ${noun}. Other sprites cannot read them.`}
        </p>
      )}
    </nav>
  )
}

/** Where a page came from: the initial of its connection, or a speech
 *  mark for a page written from conversations. */
function SourceMark({
  page,
  connectionName,
}: {
  page: MemoryPageDto
  connectionName: (connectionId: string) => string
}) {
  if (page.source_connection_id == null) {
    return (
      <span className="memory-source-mark" title="From conversations">
        <MessageSquare size={14} aria-hidden />
      </span>
    )
  }
  const name = connectionName(page.source_connection_id)
  return (
    <span className="memory-source-mark" title={`From ${name}`}>
      {name.charAt(0).toUpperCase()}
    </span>
  )
}
