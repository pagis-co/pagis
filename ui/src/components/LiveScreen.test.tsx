// The live screen view: the offer/answer handshake through the
// daemon, teardown on unmount, and the fallback when the session is
// refused.

import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { LiveScreen } from './LiveScreen'

class FakeDataChannel {
  readyState = 'open'
  sent: string[] = []
  send = vi.fn((payload: string) => {
    this.sent.push(payload)
  })
}

class FakePeerConnection {
  static instances: FakePeerConnection[] = []
  configuration: RTCConfiguration | undefined
  ontrack: ((event: unknown) => void) | null = null
  onconnectionstatechange: (() => void) | null = null
  connectionState: RTCPeerConnectionState = 'new'
  closed = false
  localDescription: unknown = null
  remoteDescription: unknown = null
  channel: FakeDataChannel | null = null

  constructor(configuration?: RTCConfiguration) {
    this.configuration = configuration
    FakePeerConnection.instances.push(this)
  }

  createDataChannel = vi.fn((label: string) => {
    void label
    this.channel = new FakeDataChannel()
    return this.channel
  })
  addTransceiver = vi.fn()
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'v=0 offer' }))
  setLocalDescription = vi.fn(async (description: unknown) => {
    this.localDescription = description
  })
  setRemoteDescription = vi.fn(async (description: unknown) => {
    this.remoteDescription = description
  })
  close = vi.fn(() => {
    this.closed = true
  })
}

/** The relay answers no ICE server unless a test says otherwise. */
function stubApi(
  offer: () => Promise<{ data?: unknown; error?: unknown }>,
  ice: () => Promise<{ data?: unknown; error?: unknown }> = async () => ({
    data: { ice_servers: [] },
  }),
) {
  return { POST: vi.fn(offer), GET: vi.fn(ice) } as unknown as ApiClient
}

beforeEach(() => {
  FakePeerConnection.instances = []
  vi.stubGlobal('RTCPeerConnection', FakePeerConnection)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('LiveScreen', () => {
  it('reports a failed media connection and lets the user retry', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen api={api} agentId="ag1" agentName="Sage"
        interactive={false} fallback={<span>Last screen</span>} />,
    )
    await waitFor(() => expect(FakePeerConnection.instances).toHaveLength(1))
    const pc = FakePeerConnection.instances[0]
    await waitFor(() => expect(pc.remoteDescription).not.toBeNull())

    act(() => {
      pc.connectionState = 'failed'
      pc.onconnectionstatechange?.()
    })

    expect(await screen.findByRole('status')).toHaveProperty(
      'textContent', 'Live screen unavailable.',
    )
    expect(screen.getByText('Last screen')).toBeTruthy()
    expect(screen.queryByLabelText("Sage's live screen")).toBeNull()
    expect(pc.closed).toBe(true)

    fireEvent.click(screen.getByRole('button', { name: 'Retry live screen' }))
    await waitFor(() => expect(FakePeerConnection.instances).toHaveLength(2))
    expect(screen.queryByText('Last screen')).toBeNull()
    expect(screen.getByLabelText("Sage's live screen")).toBeTruthy()
    await waitFor(() => expect(FakePeerConnection.instances[1].remoteDescription).not.toBeNull())
  })

  it('offers through the daemon and applies the answer', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/screen/offer',
        {
          params: { path: { agent_id: 'ag1' } },
          body: { sdp: 'v=0 offer' },
        },
      ),
    )
    const pc = FakePeerConnection.instances[0]!
    expect(pc.addTransceiver).toHaveBeenCalledWith('video', {
      direction: 'recvonly',
    })
    await waitFor(() =>
      expect(pc.remoteDescription).toEqual({
        type: 'answer',
        sdp: 'v=0 answer',
      }),
    )
    expect(screen.getByLabelText("Sage's live screen")).toBeTruthy()
  })

  it("configures the relay's ICE servers before it offers", async () => {
    // The `turn` relay mints a credential per session, and a
    // peer connection takes its ICE servers only when it is made.
    const api = stubApi(
      async () => ({ data: { sdp: 'v=0 answer' } }),
      async () => ({
        data: {
          ice_servers: [
            {
              urls: ['turn:relay.example.net:3478'],
              username: '1800:abc',
              credential: 'signed',
            },
          ],
        },
      }),
    )
    render(
      <LiveScreen api={api} agentId="ag1" agentName="Sage"
        interactive={false} fallback={<span>fallback</span>} />,
    )

    await waitFor(() => expect(FakePeerConnection.instances.length).toBe(1))
    expect(api.GET).toHaveBeenCalledWith('/api/v1/screen/ice', {})
    expect(FakePeerConnection.instances[0].configuration).toEqual({
      iceServers: [
        {
          urls: ['turn:relay.example.net:3478'],
          username: '1800:abc',
          credential: 'signed',
        },
      ],
    })
  })

  it('a relay that answers nothing falls back', async () => {
    const api = stubApi(
      async () => ({ data: { sdp: 'v=0 answer' } }),
      async () => ({ error: { error: { message: 'no relay' } } }),
    )
    render(
      <LiveScreen api={api} agentId="ag1" agentName="Sage"
        interactive={false} fallback={<span>Last screen</span>} />,
    )

    expect(await screen.findByRole('status')).toHaveProperty(
      'textContent', 'Live screen unavailable.',
    )
    expect(FakePeerConnection.instances).toHaveLength(0)
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('closing the tile tears the session down', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    const view = render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )
    await waitFor(() => expect(FakePeerConnection.instances.length).toBe(1))

    view.unmount()

    expect(FakePeerConnection.instances[0].closed).toBe(true)
  })

  it('every session opens the input data channel', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )

    await waitFor(() => expect(FakePeerConnection.instances.length).toBe(1))
    expect(
      FakePeerConnection.instances[0].createDataChannel,
    ).toHaveBeenCalledWith('input')
  })

  it('while interactive, mouse and keyboard flow over the channel', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={true}
        fallback={<span>fallback</span>}
      />,
    )
    const video = await screen.findByLabelText("Sage's live screen")
    video.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 1280, height: 800 }) as DOMRect

    fireEvent.pointerDown(video, { clientX: 640, clientY: 400, button: 0 })
    fireEvent.pointerUp(video, { clientX: 640, clientY: 400, button: 0 })
    fireEvent.keyDown(video, { code: 'KeyA' })
    fireEvent.keyUp(video, { code: 'KeyA' })

    const channel = FakePeerConnection.instances[0].channel!
    const ops = channel.sent.map((payload) => JSON.parse(payload))
    expect(ops).toContainEqual({ op: 'move', x: 640, y: 400 })
    expect(ops).toContainEqual({ op: 'button', button: 'left', down: true })
    expect(ops).toContainEqual({ op: 'button', button: 'left', down: false })
    expect(ops).toContainEqual({ op: 'key', code: 'KeyA', down: true })
    expect(ops).toContainEqual({ op: 'key', code: 'KeyA', down: false })
  })

  it('while watching, input is not forwarded', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )
    const video = await screen.findByLabelText("Sage's live screen")

    fireEvent.pointerDown(video, { clientX: 10, clientY: 10, button: 0 })
    fireEvent.keyDown(video, { code: 'KeyA' })

    expect(FakePeerConnection.instances[0].channel!.sent).toEqual([])
  })

  it('a refused offer falls back', async () => {
    const api = stubApi(async () => ({
      error: { error: { code: 'computer_asleep', message: 'asleep' } },
    }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )

    expect(await screen.findByText('fallback')).toBeTruthy()
  })
})
