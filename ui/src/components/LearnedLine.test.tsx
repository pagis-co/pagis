// The learned line under one reply: the memory its Run wrote,
// with Show and Undo.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { LearnedLine } from './LearnedLine'

const committed = {
  id: '01FEED2',
  kind: 'committed',
  sha: 'abc123',
  agent_id: 'ag1',
  agent_name: 'Sage',
  scopes: ['private'],
  files: ['private/subjects/Robin Vale.md'],
  titles: ['Robin Vale'],
  message: 'Noted the contact preference.',
  run_id: 'run1',
  message_id: 'm1',
  source_scoped: false,
  reverted_sha: null,
  created_at: 1_700_000_000_000,
  action: null,
}

function stubApi(options: {
  items?: unknown[]
  revert?: () => Promise<{ data?: unknown; error?: unknown }>
}) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/memory/feed') {
        return { data: { items: options.items ?? [committed], revision: 'head1234' } }
      }
      if (path === '/api/v1/memory/file') {
        return {
          data: {
            scope: 'agent:ag1',
            path: 'subjects/Robin Vale.md',
            content: '# Robin Vale\n\nPrefers email over a phone call.',
            revision: 'head1234',
            content_kind: 'dependent_view',
            source_scoped: false,
          },
        }
      }
      throw new Error(`unexpected GET ${path}`)
    }),
    POST: vi.fn(options.revert ?? (async () => ({ data: { sha: 'revert1' } }))),
  }
}

function mount(api: ReturnType<typeof stubApi>, messageId = 'm1') {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const view = render(
    <QueryClientProvider client={queryClient}>
      <LearnedLine api={api as unknown as ApiClient} messageId={messageId} authorName="Sage" />
    </QueryClientProvider>,
  )
  return view
}

describe('LearnedLine', () => {
  it('names what the Run noted and the page it touched', async () => {
    mount(stubApi({}))

    const line = await screen.findByTestId('learned-line')
    expect(line.textContent).toContain('Sage learned · noted the contact preference on Robin Vale')
  })

  it('names a page by its title, with no path and no id', async () => {
    const conversation = {
      ...committed,
      files: ['private/subjects/conversation/01M3BJ68SX8A040PQBAR51CCE9.md'],
      titles: ['Trip to Lisbon'],
      message: 'Update memory',
    }
    mount(stubApi({ items: [conversation] }))

    const line = await screen.findByTestId('learned-line')
    expect(line.textContent).toContain('Sage learned · update memory on Trip to Lisbon')
    expect(line.textContent).not.toContain('01M3BJ68SX8A040PQBAR51CCE9')
    expect(line.textContent).not.toContain('private/')

    fireEvent.click(screen.getByRole('button', { name: 'Show' }))
    expect(await screen.findByText('Trip to Lisbon', { selector: '.learned-page-name' })).toBeTruthy()
  })

  it('renders nothing for a reply whose Run wrote no memory', async () => {
    const api = stubApi({})
    mount(api, 'm2')

    await waitFor(() => expect(api.GET).toHaveBeenCalled())
    expect(screen.queryByTestId('learned-line')).toBeNull()
  })

  it('shows the page as it reads now', async () => {
    const api = stubApi({})
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Show' }))

    expect(await screen.findByText(/Prefers email over a phone call/)).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/file', {
      params: { query: { scope: 'agent:ag1', path: 'subjects/Robin Vale.md' } },
    })
    expect(screen.getByRole('button', { name: 'Hide' })).toBeTruthy()
  })

  it('undoes the commit against the feed revision', async () => {
    const api = stubApi({})
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Undo' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/memory/commits/{sha}/revert', {
        params: { path: { sha: 'abc123' } },
        body: { expected_revision: 'head1234' },
      }),
    )
  })

  it('says why an undo failed', async () => {
    const api = stubApi({
      revert: async () => ({
        error: { error: { message: 'cannot auto-revert: later changes touch the same files' } },
      }),
    })
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Undo' }))

    expect(await screen.findByText(/later changes touch the same files/)).toBeTruthy()
  })

  it('offers no Undo once the commit is undone', async () => {
    const reverted = {
      ...committed,
      id: '01FEED3',
      kind: 'reverted',
      sha: 'revert1',
      message_id: null,
      reverted_sha: 'abc123',
    }
    mount(stubApi({ items: [reverted, committed] }))

    const line = await screen.findByTestId('learned-line')
    expect(line.textContent).toContain('Undone')
    expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull()
  })
})
