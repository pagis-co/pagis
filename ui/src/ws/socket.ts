// The hand-rolled WebSocket client: the session cookie
// authenticates the handshake, so the first frame only resumes,
// exponential backoff with full jitter, a heartbeat with a dead-man
// close, resume via `last_seq`, and the `resync` fallback when the
// daemon's replay ring cannot cover the gap. A close with code 1008
// means that the Session ended, so the client stops and does not
// reconnect.

export type SocketStatus = 'connecting' | 'online' | 'offline'

/** A parsed server envelope: `{seq, type, payload}`. */
export interface ServerFrame {
  seq?: string | null
  replay?: boolean
  type: string
  payload?: unknown
}

export interface SocketHandlers {
  /** A domain event (a frame that carries `seq`). */
  onEvent: (frame: ServerFrame) => void
  /** An ephemeral `message.delta` frame for a subscribed channel. */
  onDelta?: (frame: ServerFrame) => void
  /** An ephemeral `progress.state` frame for a subscribed channel. */
  onProgress?: (frame: ServerFrame) => void
  /** The replay ring cannot cover the gap: drop caches and refetch. */
  onResync: () => void
  onStatus: (status: SocketStatus) => void
  /** The daemon closed the socket because the Session ended. The
   *  client opens no socket again; the app shows the sign-in page. */
  onSignedOut: () => void
}

export interface SocketOptions {
  url: string
  handlers: SocketHandlers
  /** Injection point for tests; defaults to the native WebSocket. */
  createWebSocket?: (url: string) => WebSocket
  /** Injection point for deterministic jitter in tests. */
  random?: () => number
  baseDelayMs?: number
  maxDelayMs?: number
  heartbeatMs?: number
  deadManMs?: number
}

/** The close code of a socket whose Session ended: a sign-out, an
 *  Administrator who ended every Session of the Person, or the expiry.
 *  It is 1008, policy violation, so it is not a network fault. */
export const SESSION_ENDED = 1008

const BASE_DELAY_MS = 1_000
const MAX_DELAY_MS = 30_000
const HEARTBEAT_MS = 30_000
const DEAD_MAN_MS = 60_000

export class PagisSocket {
  private readonly options: Required<
    Pick<SocketOptions, 'baseDelayMs' | 'maxDelayMs' | 'heartbeatMs' | 'deadManMs'>
  > &
    SocketOptions
  private socket: WebSocket | null = null
  private lastSeq: string | null = null
  private subscribedChannel: string | null = null
  private online = false
  private attempt = 0
  private stopped = false
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null
  private deadManTimer: ReturnType<typeof setTimeout> | null = null

  constructor(options: SocketOptions) {
    this.options = {
      baseDelayMs: BASE_DELAY_MS,
      maxDelayMs: MAX_DELAY_MS,
      heartbeatMs: HEARTBEAT_MS,
      deadManMs: DEAD_MAN_MS,
      ...options,
    }
  }

  start(): void {
    this.stopped = false
    this.connect()
  }

  stop(): void {
    this.stopped = true
    this.clearTimers()
    this.socket?.close()
    this.socket = null
  }

  /**
   * Subscribe to exactly one channel (the selected one); `null` means
   * none. The subscription gates the `message.delta` and
   * `progress.state` frames and is resent after every reconnect. A new
   * subscription answers with a catch-up frame per live stream and per
   * live run.
   */
  subscribeChannel(channelId: string | null): void {
    if (channelId === this.subscribedChannel) return
    if (this.online && this.subscribedChannel !== null) {
      this.socket?.send(
        JSON.stringify({ type: 'unsubscribe', channel_id: this.subscribedChannel }),
      )
    }
    this.subscribedChannel = channelId
    if (this.online && channelId !== null) {
      this.socket?.send(JSON.stringify({ type: 'subscribe', channel_id: channelId }))
    }
  }

  private connect(): void {
    this.options.handlers.onStatus('connecting')
    const create =
      this.options.createWebSocket ?? ((url: string) => new WebSocket(url))
    const socket = create(this.options.url)
    this.socket = socket
    this.armDeadMan() // a hung connect or a silent server also reconnects

    socket.onopen = () => {
      socket.send(
        JSON.stringify({ type: 'auth', last_seq: this.lastSeq }),
      )
    }
    socket.onmessage = (event) => this.receive(String(event.data))
    socket.onclose = (event) => this.disconnected(socket, event.code)
    socket.onerror = () => socket.close()
  }

  private receive(data: string): void {
    this.armDeadMan()
    let frame: ServerFrame
    try {
      frame = JSON.parse(data) as ServerFrame
    } catch {
      return
    }
    if (frame.type === 'ready') {
      this.attempt = 0
      this.online = true
      this.startHeartbeat()
      if (this.subscribedChannel !== null) {
        this.socket?.send(
          JSON.stringify({ type: 'subscribe', channel_id: this.subscribedChannel }),
        )
      }
      this.options.handlers.onStatus('online')
      return
    }
    if (frame.type === 'resync') {
      this.options.handlers.onResync()
      return
    }
    if (frame.type === 'message.delta') {
      this.options.handlers.onDelta?.(frame)
      return
    }
    if (frame.type === 'progress.state') {
      this.options.handlers.onProgress?.(frame)
      return
    }
    if (frame.seq != null) {
      this.lastSeq = frame.seq
      this.options.handlers.onEvent(frame)
    }
  }

  private disconnected(socket: WebSocket, code: number): void {
    if (socket !== this.socket) return
    this.socket = null
    this.online = false
    this.clearTimers()
    if (this.stopped) return
    this.options.handlers.onStatus('offline')
    if (code === SESSION_ENDED) {
      // The daemon refuses every new socket of an ended Session, so a
      // reconnect would only fail again.
      this.stopped = true
      this.options.handlers.onSignedOut()
      return
    }
    const cap = Math.min(
      this.options.baseDelayMs * 2 ** this.attempt,
      this.options.maxDelayMs,
    )
    const random = this.options.random ?? Math.random
    const delay = cap * random() // full jitter
    this.attempt += 1
    this.reconnectTimer = setTimeout(() => this.connect(), delay)
  }

  private startHeartbeat(): void {
    if (this.heartbeatTimer !== null) clearInterval(this.heartbeatTimer)
    this.heartbeatTimer = setInterval(() => {
      this.socket?.send(JSON.stringify({ type: 'ping' }))
    }, this.options.heartbeatMs)
  }

  /** Close a silent socket so the reconnect path takes over. */
  private armDeadMan(): void {
    if (this.deadManTimer !== null) clearTimeout(this.deadManTimer)
    this.deadManTimer = setTimeout(() => {
      this.socket?.close()
    }, this.options.deadManMs)
  }

  private clearTimers(): void {
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer)
    if (this.heartbeatTimer !== null) clearInterval(this.heartbeatTimer)
    if (this.deadManTimer !== null) clearTimeout(this.deadManTimer)
    this.reconnectTimer = null
    this.heartbeatTimer = null
    this.deadManTimer = null
  }
}
