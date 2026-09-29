// The listen control on a `call` block (ADR-0020): one press
// opens the socket and plays the frames, a second press closes it, and
// a call that ended says so. Nothing here signals the Remote Party.

import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import type { Speaker } from '../ws/listen'
import { ListenLive } from './ListenLive'

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  sent: string[] = []
  closed = false
  binaryType = 'blob'
  onopen: (() => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null

  constructor(public url: string) {
    FakeWebSocket.instances.push(this)
  }

  send(data: string) {
    this.sent.push(data)
  }

  close() {
    if (this.closed) return
    this.closed = true
    this.onclose?.()
  }

  open() {
    this.onopen?.()
  }

  frame(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) })
  }

  audio(bytes: number[]) {
    this.onmessage?.({ data: new Uint8Array(bytes).buffer })
  }
}

const played: Float32Array[] = []
let closedSpeakers = 0

function speaker(): Speaker {
  return {
    play: (samples) => played.push(samples),
    close: () => {
      closedSpeakers += 1
    },
  }
}

function mount() {
  render(
    <ListenLive
      callId="call-1"
      deps={{
        createWebSocket: (url) => new FakeWebSocket(url) as unknown as WebSocket,
        createSpeaker: speaker,
      }}
    />,
  )
  const socket = () =>
    FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
  return {
    press: () => fireEvent.click(screen.getByRole('button')),
    socket,
    // A frame from the daemon changes what the control shows.
    deliver: (send: (socket: FakeWebSocket) => void) =>
      act(() => {
        send(socket())
      }),
  }
}

beforeEach(() => {
  FakeWebSocket.instances = []
  played.length = 0
  closedSpeakers = 0
})

describe('the listen control', () => {
  it('opens the call socket and plays the frames it hears', () => {
    const view = mount()

    view.press()
    view.deliver((socket) => socket.open())
    view.deliver((socket) => socket.frame({ type: 'ready', codec: 'PCMU' }))
    view.deliver((socket) => socket.audio([0xff, 0xff]))

    expect(view.socket().url).toContain('/api/v1/calls/call-1/listen')
    expect(played).toHaveLength(1)
    expect(Array.from(played[0])).toEqual([0, 0])
    expect(screen.getByRole('button').textContent).toBe('Stop listening')
  })

  it('closes the socket and the speakers on a second press', () => {
    const view = mount()
    view.press()
    view.deliver((socket) => socket.open())
    view.deliver((socket) => socket.frame({ type: 'ready', codec: 'PCMU' }))

    view.press()

    expect(view.socket().closed).toBe(true)
    expect(closedSpeakers).toBe(1)
    expect(screen.getByRole('button').textContent).toBe('Listen')
  })

  it('says the call ended when it ends', () => {
    const view = mount()
    view.press()
    view.deliver((socket) => socket.open())
    view.deliver((socket) => socket.frame({ type: 'ready', codec: 'PCMU' }))

    view.deliver((socket) =>
      socket.frame({ type: 'ended', reason: 'remote_hangup' }),
    )

    expect(screen.getByText('Call ended')).toBeDefined()
  })

  it('shows why a call that is not live cannot be heard', () => {
    const view = mount()
    view.press()
    view.deliver((socket) => socket.open())

    view.deliver((socket) =>
      socket.frame({
        type: 'error',
        code: 'not_found',
        message: 'no call of that id is live',
      }),
    )

    expect(screen.getByText('no call of that id is live')).toBeDefined()
  })
})
