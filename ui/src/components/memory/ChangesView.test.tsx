// The Changes view of one scope: one row per commit with the
// Agent, the sentence, the source and the pages; filters by source; a
// row opens to the before and after; Open the Run and a one-tap Revert.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { ChangesView } from './ChangesView'
import type { MemoryPageDto } from './pages'

const NOW = Date.now()

const pages: MemoryPageDto[] = [
  {
    scope: 'agent:ag1',
    path: 'subjects/gmail/t1.md',
    excerpt: '',
    title: 'Priya Sharma',
    kind: 'Person',
    changed_at: NOW,
    changed_by: 'Sage',
  },
]

const feed = [
  {
    id: 'e4',
    kind: 'schedule',
    sha: '',
    agent_id: 'ag1',
    agent_name: 'Sage',
    scopes: ['private'],
    files: ['subjects/gmail/t1.md'],
    message: 'Send the revised quote',
    source_scoped: false,
    created_at: NOW,
    source_kind: null,
  },
  {
    id: 'e3',
    kind: 'committed',
    sha: 'sha-3',
    agent_id: 'ag1',
    agent_name: 'Sage',
    scopes: ['private'],
    files: ['private/subjects/gmail/t1.md', 'private/MEMORY.md'],
    message: 'Added the Friday deadline',
    run_id: 'run-3',
    source_scoped: false,
    created_at: NOW - 1000,
    source_kind: 'thread',
  },
  {
    id: 'e2',
    kind: 'reverted',
    sha: 'sha-2',
    agent_id: 'ag1',
    agent_name: null,
    scopes: ['private'],
    files: ['private/MEMORY.md'],
    message: '',
    reverted_sha: 'sha-1',
    source_scoped: false,
    created_at: NOW - 2000,
    source_kind: 'revert',
  },
  {
    id: 'e1',
    kind: 'committed',
    sha: 'sha-1',
    agent_id: 'ag1',
    agent_name: 'Sage',
    scopes: ['private'],
    files: ['private/MEMORY.md'],
    message: 'Learned your seat preference',
    run_id: 'run-1',
    source_scoped: false,
    created_at: NOW - 3000,
    source_kind: 'sync',
  },
]

const diff = {
  sha: 'sha-3',
  message: 'Added the Friday deadline',
  author: 'Sage',
  committed_at: NOW - 1000,
  files: [
    {
      scope: 'agent:ag1',
      path: 'subjects/gmail/t1.md',
      hunks: [
        {
          old_start: 3,
          old_lines: ['Quote due date not yet known.'],
          new_start: 3,
          new_lines: ['She asked for the revised quote by Friday.'],
        },
      ],
    },
  ],
}

type Query = { scope?: string; kind?: string }

function mount({ revert = { data: { sha: 'sha-5' } } as unknown } = {}) {
  const api = {
    GET: vi.fn(async (path: string, options?: { params?: { query?: Query } }) => {
      const query = options?.params?.query ?? {}
      if (path === '/api/v1/memory/feed') {
        const items =
          query.kind === undefined ? feed : feed.filter((item) => item.source_kind === query.kind)
        return { data: { items, revision: 'rev-9' } }
      }
      if (path === '/api/v1/memory/commits/{sha}/diff') return { data: diff }
      return { data: { items: [] } }
    }),
    POST: vi.fn(async () => revert),
  }
  const onOpenPage = vi.fn()
  const onOpenRun = vi.fn()
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <ChangesView
        api={api as unknown as ApiClient}
        scope="agent:ag1"
        owner={{ name: 'Sage' }}
        pages={pages}
        onOpenPage={onOpenPage}
        onOpenRun={onOpenRun}
      />
    </QueryClientProvider>,
  )
  return { api, onOpenPage, onOpenRun }
}

async function row(text: string): Promise<HTMLElement> {
  return (await screen.findByText(text)).closest('[role="listitem"]') as HTMLElement
}

describe('the Changes view', () => {
  it('lists one row per commit with the Agent, the sentence, the source and the pages', async () => {
    const { api } = mount()

    expect(screen.getByRole('heading', { name: 'Changes to Sage’s memory' })).toBeTruthy()
    const today = await screen.findByRole('list', { name: 'Today' })
    expect(within(today).getAllByRole('listitem')).toHaveLength(3)
    const commit = await row('added the Friday deadline')
    expect(within(commit).getByText('Sage')).toBeTruthy()
    expect(commit.textContent).toContain('from a thread')
    expect(within(commit).getByRole('button', { name: 'Priya Sharma' })).toBeTruthy()
    expect(within(commit).getByRole('button', { name: 'MEMORY.md' })).toBeTruthy()
    const revert = await row('reverted a memory change')
    expect(within(revert).getByText('You')).toBeTruthy()
    expect(screen.queryByText(/revised quote/)).toBeNull()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/feed', {
      params: { query: { scope: 'agent:ag1' } },
    })
  })

  it('asks the feed for one source kind', async () => {
    const { api } = mount()
    await row('added the Friday deadline')

    const filters = screen.getByRole('group', { name: 'Filters' })
    fireEvent.click(within(filters).getByRole('button', { name: 'From sync' }))

    expect(
      within(filters).getByRole('button', { name: 'From sync' }).getAttribute('aria-pressed'),
    ).toBe('true')
    await waitFor(() => expect(screen.queryByText('added the Friday deadline')).toBeNull())
    expect(await screen.findByText('learned your seat preference')).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/feed', {
      params: { query: { scope: 'agent:ag1', kind: 'sync' } },
    })
  })

  it('opens the page a chip names', async () => {
    const { onOpenPage } = mount()

    fireEvent.click(within(await row('added the Friday deadline')).getByRole('button', { name: 'Priya Sharma' }))

    expect(onOpenPage).toHaveBeenCalledWith('subjects/gmail/t1.md')
  })

  it('opens a row to the before and after of the commit', async () => {
    const { api } = mount()

    const commit = await row('added the Friday deadline')
    const toggle = within(commit).getByRole('button', { name: /added the Friday deadline/ })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(toggle)

    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(await within(commit).findByText('Quote due date not yet known.')).toBeTruthy()
    expect(within(commit).getByText('She asked for the revised quote by Friday.')).toBeTruthy()
    expect(within(commit).getByText('Priya Sharma · before')).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/commits/{sha}/diff', {
      params: { path: { sha: 'sha-3' } },
    })
  })

  it('opens the Run of a commit', async () => {
    const { onOpenRun } = mount()

    fireEvent.click(within(await row('added the Friday deadline')).getByRole('button', { name: 'Open the Run' }))

    expect(onOpenRun).toHaveBeenCalledWith('run-3')
  })

  it('reverts in one tap and never asks', async () => {
    const { api } = mount()

    fireEvent.click(within(await row('added the Friday deadline')).getByRole('button', { name: 'Revert' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/memory/commits/{sha}/revert', {
        params: { path: { sha: 'sha-3' } },
        body: { expected_revision: 'rev-9' },
      }),
    )
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('marks a reverted commit and a revert without actions', async () => {
    mount()

    for (const text of ['learned your seat preference', 'reverted a memory change']) {
      const reverted = await row(text)
      expect(within(reverted).getByText('Reverted')).toBeTruthy()
      expect(within(reverted).queryByRole('button', { name: 'Revert' })).toBeNull()
      expect(within(reverted).queryByRole('button', { name: 'Open the Run' })).toBeNull()
    }
  })

  it('shows why a revert failed on its row', async () => {
    mount({
      revert: {
        error: { error: { message: 'cannot auto-revert: later changes touch the same files' } },
      },
    })

    const commit = await row('added the Friday deadline')
    fireEvent.click(within(commit).getByRole('button', { name: 'Revert' }))

    expect((await within(commit).findByRole('alert')).textContent).toContain('later changes')
  })
})
