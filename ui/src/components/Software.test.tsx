// The Software destination (ADR-0022): the list carries the
// author, the tool count and the open Contributions; the detail
// carries the tools, the Versions, the origin of a Fork, and the
// Contributions with the patch collapsed until the reader opens it.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { useComposerDraft } from '../state/composerDraft'
import { Software } from './Software'

const weather = {
  name: 'weather',
  description: 'The forecast package.',
  latest_version: 'v2',
  author_agent_id: 'ag1',
  author_name: 'Sage',
  keywords: ['forecast'],
  tool_count: 1,
  open_contributions: 1,
  updated_at: 1_699_000_000_000,
}

const fork = {
  ...weather,
  name: 'weather-bo',
  author_name: 'Bo',
  open_contributions: 0,
}

const contribution = {
  id: 'c_12',
  package: 'weather',
  fork_package: 'weather-bo',
  base_version: 'v1',
  fork_version: 'v1',
  summary: 'print two, not one',
  status: 'open',
  outcome_reason: null,
  created_at: 1_699_000_000_000,
  closed_at: null,
}

const detail = {
  name: 'weather',
  description: 'The forecast package.',
  latest_version: 'v2',
  author_agent_id: 'ag1',
  author_name: 'Sage',
  keywords: ['forecast'],
  origin_package: null,
  origin_version: null,
  tools: [{ name: 'forecast', description: 'The forecast of one city.' }],
  versions: [
    { version: 'v2', notes: 'hourly forecast', published_at: 1_699_000_000_000 },
    { version: 'v1', notes: 'first cut', published_at: 1_698_000_000_000 },
  ],
  contributions: [contribution],
  created_at: 1_698_000_000_000,
  updated_at: 1_699_000_000_000,
}

const agent = { id: 'ag1', name: 'Sage', job: 'assistant', status: 'active' }
const dm = { id: 'ch-dm', title: 'Sage', kind: 'dm', agent_ids: ['ag1'], user_member: true }

function stubApi(overrides: { detail?: unknown; packages?: unknown[] } = {}) {
  return {
    GET: vi.fn(async (path: string) => {
      switch (path) {
        case '/api/v1/agents':
          return { data: { items: [agent] } }
        case '/api/v1/channels':
          return { data: { items: [dm] } }
        case '/api/v1/software':
          return { data: { items: overrides.packages ?? [weather, fork] } }
        case '/api/v1/software/{name}':
          return { data: overrides.detail ?? detail }
        case '/api/v1/software/{name}/contributions/{contribution_id}':
          return {
            data: {
              ...contribution,
              patch: '--- a/bin/forecast.py\n+++ b/bin/forecast.py\n+print(2)\n',
            },
          }
        default:
          throw new Error(`unexpected GET ${path}`)
      }
    }),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  const onOpenChannel = vi.fn()
  render(
    <QueryClientProvider client={queryClient}>
      <Software
        api={api as unknown as ApiClient}
        onClose={vi.fn()}
        onOpenChannel={onOpenChannel}
      />
    </QueryClientProvider>,
  )
  return { onOpenChannel }
}

describe('Software', () => {
  it('lists every package with its author, tools and open Contributions', async () => {
    mount(stubApi())

    expect(await screen.findByText('weather')).toBeTruthy()
    expect(screen.getByText('Sage')).toBeTruthy()
    expect(screen.getAllByText('v2').length).toBe(2)
    expect(screen.getAllByText('1 tool').length).toBe(2)
    expect(screen.getByText('1 open contribution')).toBeTruthy()
    expect(screen.getByText('weather-bo')).toBeTruthy()
  })

  it('the detail carries the tools, the Versions and the Contributions', async () => {
    mount(stubApi())

    fireEvent.click(await screen.findByText('weather'))

    expect(await screen.findByText(/weather__forecast/)).toBeTruthy()
    expect(screen.getByText(/hourly forecast/)).toBeTruthy()
    expect(screen.getByText(/first cut/)).toBeTruthy()
    expect(screen.getByText('print two, not one')).toBeTruthy()
    expect(
      screen.getByText('weather-bo v1 against weather v1'),
    ).toBeTruthy()
  })

  it('a Fork names the package it comes from', async () => {
    mount(
      stubApi({
        detail: {
          ...detail,
          name: 'weather-bo',
          origin_package: 'weather',
          origin_version: 'v1',
          contributions: [],
        },
      }),
    )

    fireEvent.click(await screen.findByText('weather-bo'))

    expect(await screen.findByText('weather v1')).toBeTruthy()
    expect(screen.getByText('No Contribution yet.')).toBeTruthy()
  })

  it('the patch is collapsed until the reader opens it', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByText('weather'))
    const open = await screen.findByRole('button', { name: 'Show patch' })
    expect(open.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByText(/\+print\(2\)/)).toBeNull()

    fireEvent.click(open)

    // The patch on screen is the proof that the read landed, so the
    // call it came from needs no wait of its own.
    expect(await screen.findByText(/\+print\(2\)/)).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith(
      '/api/v1/software/{name}/contributions/{contribution_id}',
      { params: { path: { name: 'weather', contribution_id: 'c_12' } } },
    )
  })

  it('the list says in one line what a package is', async () => {
    mount(stubApi())

    expect(
      await screen.findByText(
        'Small programs your sprites wrote, so the same job runs the same way again.',
      ),
    ).toBeTruthy()
  })

  it('every package shows the face of the agent that wrote it', async () => {
    mount(stubApi())

    await screen.findByText('weather')
    const faces = document.querySelectorAll('.software-row .ui-avatar')
    expect(faces.length).toBe(2)
    expect(faces[0].querySelector('[role=img]')?.getAttribute('aria-label')).toBe('Sage, Pixie avatar')
    expect(faces[1].querySelector('[role=img]')?.getAttribute('aria-label')).toBe('Bo, Pixie avatar')
  })

  it('an empty list asks an agent to write one, in that agent DM', async () => {
    useComposerDraft.setState({ byScope: {} })
    const { onOpenChannel } = mount(stubApi({ packages: [] }))

    fireEvent.pointerDown(
      await screen.findByRole('button', { name: 'Ask a sprite to set one up' }),
      new PointerEvent('pointerdown', { bubbles: true, button: 0 }),
    )
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Sage' }))

    await waitFor(() => expect(onOpenChannel).toHaveBeenCalledWith('ch-dm'))
    expect(useComposerDraft.getState().byScope['ch-dm']).toBe(
      'Please write a small software package for me. The job it must do is:',
    )
  })
})
