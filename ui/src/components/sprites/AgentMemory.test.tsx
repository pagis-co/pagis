// The Memory tab of an Agent profile: what the Agent holds,
// what it learns from and its last three changes, each with a link
// into the Memory place.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { AgentDto, ApiClient } from '../../api/client'
import { AgentMemory } from './AgentMemory'

const sage = {
  id: 'ag1',
  name: 'Sage',
  job: 'general assistant',
  personality: '',
  status: 'active',
} as AgentDto
const clown = { ...sage, id: 'ag2', name: 'Clown' } as AgentDto

function change(id: string, message: string, extra: Record<string, unknown> = {}) {
  return {
    id,
    kind: 'committed',
    source_kind: 'thread',
    sha: `sha-${id}`,
    agent_id: 'ag1',
    agent_name: 'Sage',
    scopes: ['private'],
    files: [],
    message,
    run_id: `run-${id}`,
    source_scoped: false,
    reverted_sha: null,
    created_at: Date.now(),
    action: null,
    ...extra,
  }
}

type Query = { params?: { query?: Record<string, unknown>; path?: Record<string, string> } }

function stubApi() {
  return {
    GET: vi.fn(async (path: string, options?: Query): Promise<{ data: unknown }> => {
      const query = options?.params?.query ?? {}
      if (path === '/api/v1/agents') return { data: { items: [sage, clown] } }
      if (path === '/api/v1/memory/pages/counts' && query.scope === 'agent:ag1') {
        return {
          data: { pages: 3, procedures: 1, authors: [{ agent_id: 'ag1', name: 'Sage', pages: 3 }] },
        }
      }
      if (path === '/api/v1/memory/pages/counts' && query.scope === 'shared') {
        return {
          data: {
            pages: 3,
            procedures: 0,
            authors: [
              { agent_id: 'ag1', name: 'Sage', pages: 1 },
              { agent_id: 'ag2', name: 'Clown', pages: 1 },
              { agent_id: null, name: 'User', pages: 1 },
            ],
          },
        }
      }
      if (path === '/api/v1/agents/{agent_id}/sync-connections') {
        return {
          data: {
            items: [
              {
                connection_id: 'cn1',
                display_name: 'Google · personal',
                provider: 'google',
                resource: 'gmail',
                enabled: true,
                caught_up: true,
                rule_count: 4,
              },
            ],
          },
        }
      }
      if (path === '/api/v1/settings/phone-numbers') {
        return { data: { items: [{ agent_id: 'ag1', e164: '+14085550142' }] } }
      }
      if (path === '/api/v1/memory/feed') {
        return {
          data: {
            items: [
              change('e3', 'added the Friday deadline'),
              change('e2', 'found the Cascade Link quote', {
                files: ['private/subjects/broadband.md'],
              }),
              change('e1', 'learned your contact preference', { kind: 'reverted', run_id: null }),
            ],
            revision: 'rev1',
          },
        }
      }
      return { data: { items: [] } }
    }),
    POST: vi.fn(async () => ({ data: { sha: 'undo' } })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const handlers = {
    onOpenMemory: vi.fn(),
    onOpenSyncSettings: vi.fn(),
    onOpenRun: vi.fn(),
  }
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <AgentMemory api={api as unknown as ApiClient} agent={sage} {...handlers} />
    </QueryClientProvider>,
  )
  return handlers
}

describe('the Memory tab of an agent profile', () => {
  it('counts the private pages, the procedures and the shared pages it wrote', async () => {
    const handlers = mount(stubApi())

    expect(await screen.findByText('2 private pages')).toBeTruthy()
    expect(screen.getByText('1 procedure')).toBeTruthy()
    expect(await screen.findByText('3 shared pages')).toBeTruthy()
    expect(screen.getByText('with Clown; Sage wrote 1 of them')).toBeTruthy()

    await userEvent.click(screen.getByRole('button', { name: 'Open in Memory' }))
    expect(handlers.onOpenMemory).toHaveBeenLastCalledWith({ scope: 'agent:ag1', view: 'pages' })
    await userEvent.click(screen.getByRole('button', { name: 'See' }))
    expect(handlers.onOpenMemory).toHaveBeenLastCalledWith({
      scope: 'agent:ag1',
      view: 'procedures',
    })
    await userEvent.click(screen.getByRole('button', { name: 'Open shared' }))
    expect(handlers.onOpenMemory).toHaveBeenLastCalledWith({ scope: 'shared', view: 'pages' })
  })

  it('names the connections it is responsible for, its desk line and its conversations', async () => {
    const api = stubApi()
    const handlers = mount(api)

    expect(await screen.findByText('Google · personal')).toBeTruthy()
    expect(screen.getByText('Responsible sprite · up to date · 4 rules')).toBeTruthy()
    expect(await screen.findByText('Desk line +1 408 555 0142')).toBeTruthy()
    expect(screen.getByText('Its conversations')).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/agents/{agent_id}/sync-connections', {
      params: { path: { agent_id: 'ag1' } },
    })

    await userEvent.click(screen.getByRole('button', { name: 'Sync settings' }))
    expect(handlers.onOpenSyncSettings).toHaveBeenCalled()
  })

  it('shows the last three changes and links the full Changes view', async () => {
    const api = stubApi()
    const handlers = mount(api)

    expect(await screen.findByText('added the Friday deadline')).toBeTruthy()
    expect(screen.getByText('found the Cascade Link quote')).toBeTruthy()
    expect(screen.getByText('subjects/broadband.md')).toBeTruthy()
    expect(screen.getByText('Reverted')).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/feed', {
      params: { query: { scope: 'agent:ag1', limit: 3 } },
    })

    await userEvent.click(screen.getAllByRole('button', { name: 'Open the Run' })[0]!)
    expect(handlers.onOpenRun).toHaveBeenCalledWith('run-e3')

    await userEvent.click(screen.getAllByRole('button', { name: 'Revert' })[0]!)
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/memory/commits/{sha}/revert', {
        params: { path: { sha: 'sha-e3' } },
        body: { expected_revision: 'rev1' },
      }),
    )

    await userEvent.click(screen.getByRole('button', { name: 'All of Sage’s changes in Memory' }))
    expect(handlers.onOpenMemory).toHaveBeenLastCalledWith({ scope: 'agent:ag1', view: 'changes' })
  })

  it('says so when the agent has no change yet', async () => {
    const api = stubApi()
    const get = api.GET.getMockImplementation()!
    api.GET.mockImplementation(async (path: string, options?: Query) =>
      path === '/api/v1/memory/feed'
        ? { data: { items: [], revision: null } }
        : get(path, options),
    )
    mount(api)

    expect(await screen.findByText('Sage has changed no memory yet.')).toBeTruthy()
  })
})
