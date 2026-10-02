// The computer tile: the state, the preview and the Wake button; an
// awake tile is a live WebRTC view that expands to a full-page view; a
// stale-image wake surfaces the daemon's refusal.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { AgentDto, ApiClient } from '../api/client'
import { useTakeoverCountdowns } from '../state/stores'
import { ComputerTile } from './Computers'

const agent = { id: 'ag1', name: 'Sage', job: 'general assistant', status: 'active' }

function stubApi(options: {
  computer?: { state: string; percent?: number | null; holder?: string; exit?: string | null }
  wake?: () => Promise<{ data?: unknown; error?: unknown }>
}) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents') return { data: { items: [agent] } }
      if (path === '/api/v1/agents/{agent_id}/computer') {
        return {
          data: {
            holder: 'agent',
            ...(options.computer ?? { state: 'off', percent: null }),
          },
        }
      }
      // The Media Relay of a local installation needs no ICE server.
      if (path === '/api/v1/screen/ice') return { data: { ice_servers: [] } }
      throw new Error(`unexpected GET ${path}`)
    }),
    POST: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents/{agent_id}/screen/offer') {
        return { data: { sdp: 'v=0 answer' } }
      }
      if (path === '/api/v1/agents/{agent_id}/screen/takeover') {
        return { data: { state: 'awake', percent: null, holder: 'user' } }
      }
      if (path === '/api/v1/agents/{agent_id}/screen/handback') {
        return { data: { state: 'awake', percent: null, holder: 'agent' } }
      }
      if (options.wake) return options.wake()
      return { data: { state: 'pulling', percent: 0 } }
    }),
  }
}

class FakePeerConnection {
  ontrack: ((event: unknown) => void) | null = null
  addTransceiver = vi.fn()
  createDataChannel = vi.fn(() => ({ readyState: 'connecting', send: vi.fn() }))
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'v=0 offer' }))
  setLocalDescription = vi.fn(async () => {})
  setRemoteDescription = vi.fn(async () => {})
  close = vi.fn()
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <ComputerTile api={api as unknown as ApiClient} agent={agent as unknown as AgentDto} />
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  // The preview endpoint is a plain authed fetch; no preview stored.
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
  vi.stubGlobal('RTCPeerConnection', FakePeerConnection)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('ComputerTile', () => {
  it('shows a sleeping tile with a Wake button and no screen', async () => {
    mount(stubApi({}))

    expect(await screen.findByText('Sage')).toBeTruthy()
    expect(screen.getByText('Asleep')).toBeTruthy()
    expect(screen.getByText('Wake')).toBeTruthy()
    expect(await screen.findByText('No screen yet')).toBeTruthy()
  })

  it('wake posts to the wake endpoint', async () => {
    const api = stubApi({})
    mount(api)

    fireEvent.click(await screen.findByText('Wake'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/computer/wake',
        { params: { path: { agent_id: 'ag1' } } },
      ),
    )
  })

  /** The exit in use (ADR-0029) is the daemon's own line, such as
   *  "exit: Air" or "exit: server". A Computer in Direct mode has none. */
  it('an awake tile names the exit in use', async () => {
    mount(stubApi({ computer: { state: 'awake', percent: null, exit: 'exit: Air' } }))

    expect(await screen.findByText('exit: Air')).toBeTruthy()
  })

  it('a tile in Direct mode names no exit', async () => {
    mount(stubApi({ computer: { state: 'awake', percent: null, exit: null } }))

    expect(await screen.findByLabelText("Sage's live screen")).toBeTruthy()
    expect(screen.queryByText(/^exit:/)).toBeNull()
  })

  it('a pulling computer shows progress and loses the Wake button', async () => {
    mount(stubApi({ computer: { state: 'pulling', percent: 40 } }))

    expect(
      await screen.findByText('Downloading the computer… 40%'),
    ).toBeTruthy()
    expect(screen.queryByText('Wake')).toBeNull()
  })

  it('an awake tile is a live view that expands to the full page', async () => {
    const api = stubApi({ computer: { state: 'awake', percent: null } })
    mount(api)

    expect(await screen.findByLabelText("Sage's live screen")).toBeTruthy()
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/screen/offer',
        expect.objectContaining({ params: { path: { agent_id: 'ag1' } } }),
      ),
    )

    fireEvent.click(screen.getByLabelText("Expand Sage's screen"))
    expect(screen.getByLabelText('Fold the screen view')).toBeTruthy()

    fireEvent.click(screen.getByLabelText('Fold the screen view'))
    expect(screen.queryByLabelText('Fold the screen view')).toBeNull()
    // The live view survives the fold: still one session, never closed.
    expect(screen.getByLabelText("Sage's live screen")).toBeTruthy()
  })

  it('the expanded view offers Take over and posts the takeover', async () => {
    const api = stubApi({ computer: { state: 'awake', percent: null } })
    mount(api)

    await screen.findByLabelText("Sage's live screen")
    fireEvent.click(screen.getByLabelText("Expand Sage's screen"))

    fireEvent.click(screen.getByText('Take over'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/screen/takeover',
        { params: { path: { agent_id: 'ag1' } } },
      ),
    )
  })

  it('while the user holds, the expanded view offers Hand back', async () => {
    const api = stubApi({
      computer: { state: 'awake', percent: null, holder: 'user' },
    })
    mount(api)

    await screen.findByLabelText("Sage's live screen")
    fireEvent.click(screen.getByLabelText("Expand Sage's screen"))

    expect(screen.queryByText('Take over')).toBeNull()
    fireEvent.click(screen.getByText('Hand back'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/screen/handback',
        { params: { path: { agent_id: 'ag1' } } },
      ),
    )
  })

  it('while the daemon fills a login, neither switch button appears', async () => {
    const api = stubApi({
      computer: { state: 'awake', percent: null, holder: 'daemon' },
    })
    mount(api)

    await screen.findByLabelText("Sage's live screen")
    fireEvent.click(screen.getByLabelText("Expand Sage's screen"))

    expect(screen.queryByText('Take over')).toBeNull()
    expect(screen.queryByText('Hand back')).toBeNull()
    expect(screen.getByText('Pagis is filling a saved login')).toBeTruthy()
  })

  it('the frame says who is in control and holds the countdown', async () => {
    const api = stubApi({
      computer: { state: 'awake', percent: null, holder: 'user' },
    })
    mount(api)

    await screen.findByLabelText("Sage's live screen")
    fireEvent.click(screen.getByLabelText("Expand Sage's screen"))
    useTakeoverCountdowns.getState().start('ag1', 15)

    const frame = await screen.findByTestId('screen-frame')
    expect(frame.dataset.holder).toBe('user')
    expect(screen.getByText('You are in control')).toBeTruthy()
    const countdown = await screen.findByText(
      /Handing control back in 15 s/,
    )
    expect(frame.contains(countdown)).toBe(true)
  })

  it('a stale-image refusal surfaces the daemon message', async () => {
    const api = stubApi({
      wake: async () => ({
        error: {
          error: {
            code: 'image_version_mismatch',
            message: 'the local computer image reports version "9.9.9"',
          },
        },
      }),
    })
    mount(api)

    fireEvent.click(await screen.findByText('Wake'))

    expect(await screen.findByText(/reports version "9.9.9"/)).toBeTruthy()
  })
})
