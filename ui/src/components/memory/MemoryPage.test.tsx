// The Memory page: the scope switcher opens on the Chief of
// Sprites, the list groups the pages by change time, and the page shows
// what the daemon holds, its history with Revert, and its sources. A
// shared page shows who reads it and has no Timeline.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { useComposerDraft } from '../../state/composerDraft'
import { externalImages } from '../../test/images'
import { threadScope } from '../../timeline'
import { MemoryPage, type MemoryPageProps } from './MemoryPage'

const NOW = Date.now()

const agents = [
  { id: 'ag1', name: 'Sage', job: 'assistant', personality: '', status: 'active' },
  { id: 'ag2', name: 'Clown', job: 'jokes', personality: '', status: 'active' },
]

const channels = [
  { id: 'ch1', workspace_id: 'ws1', kind: 'dm', title: 'Sage', agent_ids: ['ag1'], user_member: true },
  { id: 'ch2', workspace_id: 'ws1', kind: 'dm', title: 'Clown', agent_ids: ['ag2'], user_member: true },
]

const SUBJECT = [
  '# Person: Priya Sharma',
  '',
  'Head of Procurement at Northwind.',
  '',
  '## Facts',
  '',
  '| Claim | Kind | Source reference |',
  '| --- | --- | --- |',
  '| priya@northwind.co | Email | gmail:con_1:m1:1 |',
  '',
  '## Schedules',
  '',
  '| When | What for | Schedule id |',
  '| --- | --- | --- |',
  `| ${NOW + 86_400_000} | Send the revised quote | sch_1 |`,
  '',
  '---',
  '',
  '## Timeline',
  '',
  '### Entry',
  '',
  'Source reference: `gmail:con_1:m1:1`',
  '',
  `Source time: ${NOW}`,
  '',
  '> Priya asks for the quote by Friday.',
  '',
].join('\n')

const pages: Record<string, unknown[]> = {
  'agent:ag1': [
    {
      scope: 'agent:ag1',
      path: 'subjects/gmail/t1.md',
      title: 'Priya Sharma',
      kind: 'Person',
      source_connection_id: 'con_1',
      changed_at: NOW,
      changed_by: 'Sage',
      changed_by_agent_id: 'ag1',
    },
    {
      scope: 'agent:ag1',
      path: 'MEMORY.md',
      title: 'Memory',
      kind: null,
      source_connection_id: null,
      changed_at: NOW - 30 * 86_400_000,
      changed_by: 'Sage',
      changed_by_agent_id: 'ag1',
    },
  ],
  'agent:ag2': ['one', 'two', 'three'].map((name, index) => ({
    scope: 'agent:ag2',
    path: `${name}.md`,
    title: `Joke ${name}`,
    kind: null,
    source_connection_id: null,
    changed_at: NOW - index * 60_000,
    changed_by: 'Clown',
    changed_by_agent_id: 'ag2',
  })),
  shared: [
    {
      scope: 'shared',
      path: 'household.md',
      title: 'Household',
      kind: 'Preferences',
      source_connection_id: null,
      changed_at: NOW,
      changed_by: 'Clown',
      changed_by_agent_id: 'ag2',
    },
  ],
}

const files: Record<string, unknown> = {
  'agent:ag1 subjects/gmail/t1.md': {
    scope: 'agent:ag1',
    path: 'subjects/gmail/t1.md',
    title: 'Priya Sharma',
    kind: 'Person',
    content: SUBJECT,
    revision: 'rev-1',
    content_kind: 'dependent_view',
    source_scoped: true,
    sources: [{ connection_id: 'con_1', resource: 'gmail', count: 3 }],
  },
  'shared household.md': {
    scope: 'shared',
    path: 'household.md',
    title: 'Household',
    kind: 'Preferences',
    content: '# Household\n\nTwo kids, ages 6 and 9.\n',
    revision: 'rev-1',
    content_kind: 'procedure',
    source_scoped: false,
    sources: [],
  },
  'shared chart.md': {
    scope: 'shared',
    path: 'chart.md',
    title: 'Chart',
    kind: null,
    content: '# Chart\n\nThe chart: ![a](https://example.com/p.png?d=secret)\n',
    revision: 'rev-1',
    content_kind: 'procedure',
    source_scoped: false,
    sources: [],
  },
}

const history = [
  {
    id: 'e2',
    kind: 'reverted',
    sha: 'sha-2',
    agent_id: null,
    agent_name: null,
    scopes: ['private'],
    files: ['private/subjects/gmail/t1.md'],
    message: 'Reverted a memory change',
    source_scoped: false,
    reverted_sha: 'sha-0',
    created_at: NOW,
    source_kind: 'revert',
  },
  {
    id: 'e1',
    kind: 'committed',
    sha: 'sha-1',
    agent_id: 'ag1',
    agent_name: 'Sage',
    scopes: ['private'],
    files: ['private/subjects/gmail/t1.md', 'private/MEMORY.md'],
    message: 'Added the Friday deadline',
    source_scoped: false,
    created_at: NOW - 3_600_000,
    source_kind: 'sync',
  },
]

type Query = { scope?: string; path?: string; kind?: string; q?: string; after?: string }
type Row = { path: string; title: string; kind: string | null }

/** The daemon gives a list in parts of this size. */
const PART = 2

/** The part of a scope that a query asks for, as the daemon narrows it. */
function listPart(query: Query) {
  const needle = (query.q ?? '').trim().toLowerCase()
  const rows = ((pages[query.scope ?? ''] ?? []) as Row[]).filter(
    (row) =>
      (query.kind === undefined || row.kind === query.kind) &&
      (row.title.toLowerCase().includes(needle) || row.path.toLowerCase().includes(needle)),
  )
  const start = query.after === undefined ? 0 : Number(query.after)
  const end = start + PART
  return {
    pages: rows.slice(start, end),
    next: end < rows.length ? String(end) : null,
    total: rows.length,
  }
}

function stubApi() {
  return {
    GET: vi.fn(async (path: string, options?: { params?: { query?: Query } }) => {
      const query = options?.params?.query ?? {}
      if (path === '/api/v1/agents') return { data: { items: agents } }
      if (path === '/api/v1/workspace') {
        return {
          data: {
            id: 'ws-1',
            name: 'Workspace',
            timezone: 'UTC',
            chief_of_staff_agent_id: 'ag1',
          },
        }
      }
      if (path === '/api/v1/channels') return { data: { items: channels } }
      if (path === '/api/v1/settings/connections') {
        return {
          data: {
            items: [{ id: 'con_1', display_name: 'Google · personal', provider: 'google' }],
          },
        }
      }
      if (path === '/api/v1/memory/pages') return { data: listPart(query) }
      if (path === '/api/v1/memory/pages/counts') {
        const rows = (pages[query.scope ?? ''] ?? []) as Row[]
        return {
          data: {
            pages: rows.length,
            procedures: rows.filter((row) => row.kind === 'Procedure').length,
            authors: [],
          },
        }
      }
      if (path === '/api/v1/memory/file') {
        return { data: files[`${query.scope} ${query.path}`] }
      }
      if (path === '/api/v1/memory/feed') {
        return { data: { items: query.scope === 'shared' ? [] : history, revision: 'rev-9' } }
      }
      return { data: { items: [] } }
    }),
    POST: vi.fn(async () => ({ data: { sha: 'sha-3' } })),
  }
}

function mount(props: Partial<MemoryPageProps> = {}) {
  const api = stubApi()
  const onChange = vi.fn()
  const onOpenChannel = vi.fn()
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <MemoryPage
        api={api as unknown as ApiClient}
        view="pages"
        onChange={onChange}
        onOpenChannel={onOpenChannel}
        onOpenRun={vi.fn()}
        onOpenNav={vi.fn()}
        {...props}
      />
    </QueryClientProvider>,
  )
  return { api, onChange, onOpenChannel }
}

function scopes() {
  return screen.findByRole('group', { name: 'Scope' })
}

beforeEach(() => {
  useComposerDraft.setState({ byScope: {} })
})

describe('the scope switcher', () => {
  it('opens on the Chief of Staff, then lists each Agent and Shared', async () => {
    mount()

    const group = await scopes()
    const sage = await within(group).findByRole('button', { name: 'Sage' })
    expect(sage.getAttribute('aria-pressed')).toBe('true')
    const names = within(group)
      .getAllByRole('button')
      .map((button) => button.textContent)
    expect(names).toEqual(['Sage', 'Clown', 'Shared'])
  })

  it('asks for another scope', async () => {
    const { onChange } = mount()

    fireEvent.click(await within(await scopes()).findByRole('button', { name: 'Shared' }))

    expect(onChange).toHaveBeenCalledWith({ scope: 'shared', view: 'pages' })
  })
})

describe('the page list', () => {
  it('groups the pages by change time with the kind and the source mark', async () => {
    mount()

    const list = await screen.findByRole('navigation', { name: 'Memory' })
    expect(await within(list).findByText('Changed today')).toBeTruthy()
    expect(within(list).getByText('Earlier')).toBeTruthy()
    const row = within(list).getByRole('button', { name: /Priya Sharma/ })
    expect(row.textContent).toContain('Person')
    expect(within(row).getByTitle('From Google · personal')).toBeTruthy()
    const conversation = within(list).getByRole('button', { name: /^Memory/ })
    expect(within(conversation).getByTitle('From conversations')).toBeTruthy()
    expect(within(list).getByText(/Sage holds 2 pages/)).toBeTruthy()
  })

  it('asks the daemon for the search, and keeps the open page', async () => {
    const { api } = mount()

    const search = await screen.findByLabelText('Search Sage’s pages')
    await screen.findByRole('button', { name: /Priya Sharma/ })
    fireEvent.change(search, { target: { value: 'memory.md' } })

    await waitFor(() => expect(screen.queryByRole('button', { name: /Priya Sharma/ })).toBeNull())
    expect(screen.getByRole('button', { name: /^Memory/ })).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/pages', {
      params: { query: { scope: 'agent:ag1', q: 'memory.md' } },
    })
    // The search narrows the list, not the page that is open.
    expect(screen.getByRole('heading', { name: 'Priya Sharma' })).toBeTruthy()
    expect(screen.getByText(/Sage holds 2 pages/)).toBeTruthy()
  })

  it('says when no page matches the search', async () => {
    mount()

    const search = await screen.findByLabelText('Search Sage’s pages')
    fireEvent.change(search, { target: { value: 'nothing like this' } })

    expect(await screen.findByText('No page matches the search.')).toBeTruthy()
  })

  it('asks the daemon for the procedures of the Procedures view', async () => {
    const { api } = mount({ view: 'procedures' })

    await screen.findByRole('navigation', { name: 'Memory' })
    await waitFor(() =>
      expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/pages', {
        params: { query: { scope: 'agent:ag1', kind: 'Procedure' } },
      }),
    )
  })

  it('loads the next part of a long list on request', async () => {
    const { api } = mount({ scope: 'agent:ag2' })

    const list = await screen.findByRole('navigation', { name: 'Memory' })
    expect(await within(list).findByRole('button', { name: /Joke two/ })).toBeTruthy()
    expect(within(list).queryByRole('button', { name: /Joke three/ })).toBeNull()
    expect(within(list).getByText(/Clown holds 3 pages/)).toBeTruthy()

    fireEvent.click(within(list).getByRole('button', { name: 'Show more pages (1 more)' }))

    expect(await within(list).findByRole('button', { name: /Joke three/ })).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/pages', {
      params: { query: { scope: 'agent:ag2', after: '2' } },
    })
    expect(within(list).queryByRole('button', { name: /Show more pages/ })).toBeNull()
  })

  it('asks for the page a row names', async () => {
    const { onChange } = mount()

    fireEvent.click(await screen.findByRole('button', { name: /^Memory/ }))

    expect(onChange).toHaveBeenCalledWith({ scope: 'agent:ag1', path: 'MEMORY.md', view: 'pages' })
  })

  it('shows the Changes of the scope beside the list, and a row opens its page', async () => {
    const { onChange } = mount({ view: 'changes' })

    expect(await screen.findByRole('heading', { name: 'Changes to Sage’s memory' })).toBeTruthy()
    expect(await screen.findByText('added the Friday deadline')).toBeTruthy()
    expect(screen.queryByRole('heading', { name: 'Priya Sharma', level: 1 })).toBeNull()

    const list = screen.getByRole('navigation', { name: 'Memory' })
    fireEvent.click(await within(list).findByRole('button', { name: /^Memory/ }))

    expect(onChange).toHaveBeenCalledWith({ scope: 'agent:ag1', path: 'MEMORY.md', view: 'pages' })
  })
})

describe('an Agent page', () => {
  it('shows the truth, the Facts, the open Schedules, the rule and the Timeline', async () => {
    mount()

    expect(await screen.findByRole('heading', { name: 'Priya Sharma', level: 1 })).toBeTruthy()
    expect(screen.getByText('Sage · private')).toBeTruthy()
    expect(screen.getByText('subjects/gmail/t1.md')).toBeTruthy()
    expect(await screen.findByText('Head of Procurement at Northwind.')).toBeTruthy()
    const facts = screen.getByRole('table', { name: 'Facts' })
    expect(within(facts).getByText('priya@northwind.co')).toBeTruthy()
    expect(screen.getByText('Send the revised quote')).toBeTruthy()
    expect(screen.getByRole('separator')).toBeTruthy()
    const timeline = screen.getByRole('list', { name: 'Timeline' })
    expect(within(timeline).getByText('Priya asks for the quote by Friday.')).toBeTruthy()
    expect(within(timeline).getByText(/gmail · Google · personal/)).toBeTruthy()
  })

  it('shows the history with Revert per commit and a mark on a revert', async () => {
    const { api } = mount()

    const rail = await screen.findByRole('list', { name: 'History of this page' })
    const commit = (await within(rail).findByText('Added the Friday deadline')).closest('[role="listitem"]') as HTMLElement
    expect(commit.textContent).toContain('2 files')
    const revert = (await within(rail).findByText('Reverted a memory change')).closest('[role="listitem"]') as HTMLElement
    expect(within(revert).getByText('Reverted')).toBeTruthy()
    expect(within(revert).queryByRole('button', { name: 'Revert' })).toBeNull()

    fireEvent.click(within(commit).getByRole('button', { name: 'Revert' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/memory/commits/{sha}/revert', {
        params: { path: { sha: 'sha-1' } },
        body: { expected_revision: 'rev-9' },
      }),
    )
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/feed', {
      params: { query: { scope: 'agent:ag1', path: 'subjects/gmail/t1.md' } },
    })
  })

  it('lists the sources of the page', async () => {
    mount()

    const sources = await screen.findByRole('list', { name: 'Sources' })
    expect(await within(sources).findByText('Google · personal')).toBeTruthy()
    expect(within(sources).getByText('3 arrivals · gmail')).toBeTruthy()
  })

  it('opens the Agent’s conversation with a draft about the page', async () => {
    const { onOpenChannel } = mount()

    // The file names the title of the page.
    await screen.findByRole('heading', { name: 'Priya Sharma' })
    fireEvent.click(await screen.findByRole('button', { name: 'Ask Sage about it' }))

    expect(onOpenChannel).toHaveBeenCalledWith('ch1')
    expect(useComposerDraft.getState().byScope[threadScope('ch1')]).toContain(
      'Priya Sharma',
    )
  })

  it('forgets the page through its source items', async () => {
    const { api } = mount()

    fireEvent.click(await screen.findByRole('button', { name: 'Forget this page' }))
    const dialog = await screen.findByRole('dialog')
    const item = within(dialog).getByRole('region', { name: /Forget gmail item m1/ })
    fireEvent.click(within(item).getByRole('button', { name: 'Preview what goes' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/knowledge/forget/preview', {
        body: {
          target: {
            kind: 'source',
            source: { workspace_id: 'ws1', connection_id: 'con_1', resource: 'gmail' },
            source_id: 'm1',
          },
        },
      }),
    )
  })
})

describe('a shared page', () => {
  it('shows who reads it and has no Timeline', async () => {
    mount({ scope: 'shared' })

    expect(await screen.findByRole('heading', { name: 'Household', level: 1 })).toBeTruthy()
    expect(screen.getByText('Shared · every sprite')).toBeTruthy()
    expect(await screen.findByText('Two kids, ages 6 and 9.')).toBeTruthy()
    expect(screen.queryByRole('list', { name: 'Timeline' })).toBeNull()
    const readers = await screen.findByRole('list', { name: 'Who reads it' })
    expect(within(readers).getByText('Sage')).toBeTruthy()
    expect(within(readers).getByText('Clown')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Forget this page' })).toBeNull()
  })

  // Each Person who opens a shared page reads it in their own browser.
  // An image that an Agent wrote into the page loads for none of them.
  it('loads no image that its prose names', async () => {
    mount({ scope: 'shared', path: 'chart.md' })

    expect(await screen.findByText(/The chart:/)).toBeTruthy()
    expect(externalImages()).toEqual([])
    expect(screen.getByRole('link', { name: 'a' }).getAttribute('href')).toBe(
      'https://example.com/p.png?d=secret',
    )
  })
})
