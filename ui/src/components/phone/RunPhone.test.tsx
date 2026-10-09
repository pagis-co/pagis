// A Run on the phone: its title, its state, the failure in plain words
// with the next step, its steps, and a way to the Agent's live screen.
// Back goes to the fixed parent: Home, or the sprite's Work it came from.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from '@tanstack/react-router'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, RunDto } from '../../api/client'
import { shellResponse } from '../../test/appStub'
import { formatClock } from '../../timeline'
import { RunPhone } from './RunPhone'

const STARTED = Date.parse('2026-10-08T09:12:00Z')

function run(fields: Partial<RunDto> = {}): RunDto {
  return {
    id: 'run-1',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    root_message_id: null,
    title: 'Check the supplier prices',
    trigger_kind: 'schedule',
    trigger_ref: null,
    hop_count: 0,
    state: 'failed',
    error: null,
    failure_kind: 'tool_failed',
    started_at: STARTED,
    ended_at: STARTED + 60_000,
    created_at: STARTED,
    duration_ms: 60_000,
    ...fields,
  }
}

function stubApi(record: RunDto, post: (path: string) => Promise<unknown> = async () => ({ data: {} })): ApiClient {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/runs/{run_id}/events') {
        return { data: { run: record, events: [], usage: { input_tokens: 0, output_tokens: 0 } } }
      }
      if (path === '/api/v1/runs/{run_id}/steps') {
        return {
          data: {
            run_id: record.id,
            state: record.state,
            worked_ms: 60_000,
            stopped: null,
            failure: null,
            steps: [
              { index: 1, label: 'Opened the supplier site', kind: 'desk', started_at: STARTED, ended_at: STARTED + 30_000, duration_ms: 30_000, screenshot_id: null, live: false },
            ],
          },
        }
      }
      return shellResponse(path)
    }),
    POST: vi.fn(post),
  } as unknown as ApiClient
}

function mount(record: RunDto, url = '/runs/run-1', post?: (path: string) => Promise<unknown>) {
  const api = stubApi(record, post)
  const history = createMemoryHistory({ initialEntries: [url] })
  const rootRoute = createRootRoute({ component: () => <Outlet /> })
  const placeholder = (path: string) =>
    createRoute({ getParentRoute: () => rootRoute, path, component: () => <p>{`the place ${path}`}</p> })
  const runRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/runs/$runId',
    component: () => <RunPhone api={api} runId={runRoute.useParams().runId} />,
  })
  const router = createRouter({
    routeTree: rootRoute.addChildren([
      runRoute,
      placeholder('/'),
      placeholder('/sprites/$agentId/work'),
      placeholder('/sprites/$agentId/desk'),
      placeholder('/c/$channelId'),
    ]),
    history,
  })
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router as never} />
    </QueryClientProvider>,
  )
  return history
}

describe('RunPhone', () => {
  it('says why a failed Run ended and what to do next', async () => {
    mount(run())

    expect(await screen.findByRole('heading', { name: 'Check the supplier prices' })).toBeTruthy()
    expect(screen.getByText('Failed')).toBeTruthy()
    expect(screen.getByText('Ended because a tool failed.')).toBeTruthy()
    expect(
      screen.getByText('Open the desk to see what the tool needs, then ask the sprite to try again.'),
    ).toBeTruthy()
  })

  it('shows no failure for a Run that is done', async () => {
    mount(run({ state: 'completed', failure_kind: null }))

    expect(await screen.findByText('Done')).toBeTruthy()
    expect(screen.queryByText(/Ended because/)).toBeNull()
  })

  it('shows each step with its label and time', async () => {
    mount(run())

    const step = (await screen.findByText('Opened the supplier site')).closest('summary') as HTMLElement
    expect(within(step).getByText(formatClock(STARTED))).toBeTruthy()
  })

  it("opens the Agent's live screen with the way back to the Run", async () => {
    const history = mount(run())

    fireEvent.click(await screen.findByRole('button', { name: 'Open the desk' }))

    await waitFor(() => expect(history.location.pathname).toBe('/sprites/agent-1/desk'))
    expect(new URLSearchParams(history.location.search).get('from')).toBe('/runs/run-1')
  })

  it('goes back to Home', async () => {
    const history = mount(run())

    fireEvent.click(await screen.findByRole('button', { name: 'Home' }))

    await waitFor(() => expect(history.location.pathname).toBe('/'))
  })

  it("goes back to the sprite's Work it came from", async () => {
    const history = mount(run(), '/runs/run-1?from=%2Fsprites%2Fagent-1%2Fwork')

    fireEvent.click(await screen.findByRole('button', { name: 'Work' }))

    await waitFor(() => expect(history.location.pathname).toBe('/sprites/agent-1/work'))
  })

  it('goes back to Home when from names another site', async () => {
    const history = mount(run(), '/runs/run-1?from=%2F%2Fevil.example')

    fireEvent.click(await screen.findByRole('button', { name: 'Home' }))

    await waitFor(() => expect(history.location.pathname).toBe('/'))
  })

  it('says why the daemon refused to retry a memory review', async () => {
    mount(run({ trigger_kind: 'review', failure_kind: null }), '/runs/run-1', async () => ({
      error: { error: { code: 'conflict', message: 'The memory review already runs.' } },
    }))

    fireEvent.click(await screen.findByRole('button', { name: 'Try again' }))

    expect((await screen.findByRole('alert')).textContent).toBe('The memory review already runs.')
  })
})
