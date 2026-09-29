// The WS client contract: a first frame that resumes with last_seq, seq tracking,
// heartbeat, dead-man close, backoff with jitter, and resync.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { PagisSocket, type ServerFrame, type SocketStatus } from './socket'

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  sent: string[] = []
  closed = false
  onopen: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  onclose: ((event: { code: number }) => void) | null = null
  onerror: (() => void) | null = null

  constructor(public url: string) {
    FakeWebSocket.instances.push(this)
  }

  send(data: string) {
    this.sent.push(data)
  }

  /** The client closes, or the connection drops: no close code arrives. */
  close() {
    this.closeWith(1006)
  }

  /** The daemon closes the socket with `code`. */
  closeWith(code: number) {
    if (this.closed) return
    this.closed = true
    this.onclose?.({ code })
  }

  // Test helpers.
  open() {
    this.onopen?.()
  }

  frame(frame: ServerFrame) {
    this.onmessage?.({ data: JSON.stringify(frame) })
  }
}

function harness(overrides: { random?: () => number } = {}) {
  const events: ServerFrame[] = []
  const deltas: ServerFrame[] = []
  const statuses: SocketStatus[] = []
  let resyncs = 0
  let signOuts = 0
  const socket = new PagisSocket({
    url: 'ws://test/api/v1/ws',
    createWebSocket: (url) => new FakeWebSocket(url) as unknown as WebSocket,
    random: overrides.random ?? (() => 1),
    handlers: {
      onEvent: (frame) => events.push(frame),
      onDelta: (frame) => deltas.push(frame),
      onResync: () => (resyncs += 1),
      onStatus: (status) => statuses.push(status),
      onSignedOut: () => (signOuts += 1),
    },
  })
  return {
    socket,
    events,
    deltas,
    statuses,
    resyncs: () => resyncs,
    signOuts: () => signOuts,
    ws: (index: number) => FakeWebSocket.instances[index],
  }
}

describe('PagisSocket', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    FakeWebSocket.instances = []
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('opens with a null last_seq, then resumes with the last seen seq', () => {
    const h = harness()
    h.socket.start()
    const first = h.ws(0)
    first.open()
    // The cookie authenticates the handshake: the frame carries
    // no credential.
    expect(JSON.parse(first.sent[0])).toEqual({ type: 'auth', last_seq: null })

    first.frame({ type: 'ready' })
    first.frame({ seq: 'evt-1', type: 'message.completed', payload: {} })
    first.frame({ seq: 'evt-2', type: 'message.completed', payload: {} })
    expect(h.events).toHaveLength(2)

    first.close()
    vi.advanceTimersByTime(1_000)
    const second = h.ws(1)
    second.open()
    expect(JSON.parse(second.sent[0])).toEqual({
      type: 'auth',
      last_seq: 'evt-2',
    })
    h.socket.stop()
  })

  it('backs off exponentially up to the cap and resets after ready', () => {
    const h = harness()
    h.socket.start()

    // Failures 1..6: delay caps at 30s (1s, 2s, 4s, 8s, 16s, 30s).
    const delays = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000]
    for (const [index, delay] of delays.entries()) {
      h.ws(index).close()
      vi.advanceTimersByTime(delay - 1)
      expect(FakeWebSocket.instances).toHaveLength(index + 1)
      vi.advanceTimersByTime(1)
      expect(FakeWebSocket.instances).toHaveLength(index + 2)
    }

    // A ready frame resets the attempt counter.
    const online = h.ws(6)
    online.open()
    online.frame({ type: 'ready' })
    online.close()
    vi.advanceTimersByTime(1_000)
    expect(FakeWebSocket.instances).toHaveLength(8)
    h.socket.stop()
  })

  it('sends a ping every heartbeat interval and closes a silent socket', () => {
    const h = harness()
    h.socket.start()
    const ws = h.ws(0)
    ws.open()
    ws.frame({ type: 'ready' })

    vi.advanceTimersByTime(30_000)
    expect(ws.sent.filter((s) => JSON.parse(s).type === 'ping')).toHaveLength(1)

    // No frame for the dead-man window: the socket is closed so the
    // reconnect path takes over.
    vi.advanceTimersByTime(30_000)
    expect(ws.closed).toBe(true)
    h.socket.stop()
  })

  it('routes message.delta frames to onDelta without touching last_seq', () => {
    const h = harness()
    h.socket.start()
    const ws = h.ws(0)
    ws.open()
    ws.frame({ type: 'ready' })
    ws.frame({
      type: 'message.delta',
      payload: { message_id: 'msg-1', seq: 1, text: 'Hel' },
    })
    expect(h.deltas).toHaveLength(1)
    expect(h.events).toHaveLength(0)

    // The next reconnect still opens with a null last_seq.
    ws.close()
    vi.advanceTimersByTime(1_000)
    const second = h.ws(1)
    second.open()
    expect(JSON.parse(second.sent[0]).last_seq).toBeNull()
    h.socket.stop()
  })

  it('subscribes the selected channel and resends it after a reconnect', () => {
    const h = harness()
    h.socket.start()
    const ws = h.ws(0)
    ws.open()
    ws.frame({ type: 'ready' })

    h.socket.subscribeChannel('ch-1')
    expect(JSON.parse(ws.sent.at(-1)!)).toEqual({
      type: 'subscribe',
      channel_id: 'ch-1',
    })

    // Switching channels unsubscribes the old one first.
    h.socket.subscribeChannel('ch-2')
    const tail = ws.sent.slice(-2).map((s) => JSON.parse(s))
    expect(tail).toEqual([
      { type: 'unsubscribe', channel_id: 'ch-1' },
      { type: 'subscribe', channel_id: 'ch-2' },
    ])

    // The subscription survives a reconnect.
    ws.close()
    vi.advanceTimersByTime(1_000)
    const second = h.ws(1)
    second.open()
    second.frame({ type: 'ready' })
    expect(JSON.parse(second.sent.at(-1)!)).toEqual({
      type: 'subscribe',
      channel_id: 'ch-2',
    })
    h.socket.stop()
  })

  it('reports resync so the app refetches everything', () => {
    const h = harness()
    h.socket.start()
    const ws = h.ws(0)
    ws.open()
    ws.frame({ type: 'ready' })
    ws.frame({ type: 'resync' })
    expect(h.resyncs()).toBe(1)
    h.socket.stop()
  })

  // The daemon closes a socket with 1008 when its Session ends: a
  // sign-out, an Administrator who ends every Session of the Person, or
  // the expiry. A new socket would be refused, so the client stops and
  // the app shows the sign-in page.
  it('treats a 1008 close as signed out and does not reconnect', () => {
    const h = harness()
    h.socket.start()
    const ws = h.ws(0)
    ws.open()
    ws.frame({ type: 'ready' })

    ws.closeWith(1008)

    expect(h.signOuts()).toBe(1)
    expect(h.statuses.at(-1)).toBe('offline')
    vi.advanceTimersByTime(120_000)
    expect(FakeWebSocket.instances).toHaveLength(1)
  })

  it('reconnects after a close that is not the end of the Session', () => {
    const h = harness()
    h.socket.start()
    const ws = h.ws(0)
    ws.open()
    ws.frame({ type: 'ready' })

    ws.closeWith(1011)

    expect(h.signOuts()).toBe(0)
    vi.advanceTimersByTime(1_000)
    expect(FakeWebSocket.instances).toHaveLength(2)
    h.socket.stop()
  })

  it('reports status transitions and stops cleanly', () => {
    const h = harness()
    h.socket.start()
    const ws = h.ws(0)
    ws.open()
    ws.frame({ type: 'ready' })
    ws.close()
    expect(h.statuses).toEqual(['connecting', 'online', 'offline'])
    vi.advanceTimersByTime(1_000)
    expect(h.statuses).toEqual(['connecting', 'online', 'offline', 'connecting'])

    h.socket.stop()
    const count = FakeWebSocket.instances.length
    vi.advanceTimersByTime(120_000)
    expect(FakeWebSocket.instances).toHaveLength(count)
  })
})
