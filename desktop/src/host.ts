// This client is the Host of the machine it runs on.
//
// A host action runs here and never in the daemon, on a local installation
// or a server.
// The client holds its authenticated socket, registers the machine, and
// runs each dispatched command as the OS user who started the client,
// which is the boundary: Pagis sandboxes nothing here, and the approval
// the person gave is what stands in its place.
//
// On a local installation this is the same machine the daemon runs on.
// On a server it is the person's own computer.

import { exec } from 'node:child_process'
import os from 'node:os'

import { isTrustedServerOrigin } from './origin'

/** What the client can do, as the daemon reads it. A machine that runs
 *  commands declares this; a client that cannot must not. */
export const SHELL_CAPABILITY = 'shell'

/** A machine that can be the Home Exit of its Person declares this: it
 *  opens the exit socket and carries the connections of the Person's
 *  Computers (`exit.ts`). */
export const EXIT_CAPABILITY = 'exit'

/** The prefix of a capability that names a Coding Harness that the
 *  machine can start, as the Harness Catalog of the daemon writes it:
 *  `harness:<id>`. A machine whose registration holds one opens the
 *  session socket (`sessions.ts`). */
export const HARNESS_CAPABILITY_PREFIX = 'harness:'

/** One command the daemon dispatched. */
export interface HostDispatch {
  id: string
  command: string
  timeout_ms: number
  /** True when an Allow Rule approved the command, and false when the
   *  person approved it on its card. */
  approved_by_rule: boolean
}

/** What the command did. */
export interface HostResult {
  exit_code: number | null
  stdout: string
  stderr: string
}

/** What the client uses of its socket. The real one is a WebSocket; a
 *  test gives its own, so the protocol is proved without a daemon. */
export interface HostSocket {
  send(frame: string): void
  onMessage(listener: (frame: string) => void): void
  /** The listener gets the close code of the socket. */
  onClose(listener: (code: number) => void): void
  close(): void
}

/** The close code of a socket whose Session ended: a sign-out, an
 *  Administrator who ended every Session of the Person, or the expiry.
 *  It is 1008, policy violation, so it is not a network fault. */
export const SESSION_ENDED = 1008

/** Runs one command and answers what it did. */
export type CommandRunner = (dispatch: HostDispatch) => Promise<HostResult>

/** The machine name a person recognizes: what the operating system calls
 *  this computer, without the `.local` a network name carries. */
export function machineName(hostname: string = os.hostname()): string {
  return hostname.replace(/\.local$/i, '') || 'this computer'
}

/** The platform name the daemon files the machine under. */
export function platformName(platform: string = process.platform): string {
  if (platform === 'darwin') return 'macos'
  if (platform === 'win32') return 'windows'
  return platform
}

/** The longest output the client sends back, in characters. The daemon
 *  caps a tool result as well; this stops the socket carrying a whole
 *  file first. */
export const MAX_OUTPUT = 100_000

function cap(text: string): string {
  if (text.length <= MAX_OUTPUT) return text
  return `${text.slice(0, MAX_OUTPUT)}\n[the output is ${text.length} characters; the first ${MAX_OUTPUT} are shown]`
}

/**
 * Run one command as the OS user.
 *
 * The command comes from a person's approval, so it is the person's own
 * command and it runs with the person's own rights. It is stopped at the
 * daemon's deadline: the daemon stops waiting then, and a command left
 * running after that answers nobody.
 *
 * A command that an Allow Rule approved runs under `/bin/sh`. The daemon
 * checked it as a POSIX shell command, and a shell such as zsh has syntax
 * that the check does not model, so the command runs in the dialect that
 * was checked. A command that the person approved on its card runs in
 * their own shell, so an alias-free command behaves as it does in their
 * terminal.
 */
export const runInShell: CommandRunner = (dispatch) =>
  new Promise((resolve) => {
    exec(
      dispatch.command,
      {
        timeout: dispatch.timeout_ms,
        maxBuffer: 8 * 1024 * 1024,
        shell: dispatch.approved_by_rule ? '/bin/sh' : process.env.SHELL || undefined,
        cwd: os.homedir(),
      },
      (error, stdout, stderr) => {
        const failed = error as (Error & { code?: number; killed?: boolean }) | null
        const code = failed === null ? 0 : typeof failed.code === 'number' ? failed.code : null
        resolve({
          exit_code: code,
          stdout: cap(stdout.toString()),
          stderr: cap(
            failed !== null && failed.killed === true
              ? `${stderr.toString()}the command was stopped at its deadline\n`
              : stderr.toString(),
          ),
        })
      },
    )
  })

/**
 * Hold one socket as the Host of this machine.
 *
 * It sends the auth frame the socket expects first, registers the
 * machine, and answers every dispatch. It sends a result for each
 * dispatch it receives, whatever the command did: a command that fails is
 * an answer the person reads, and silence would leave the sprite waiting
 * out the daemon's deadline for nothing.
 */
export class HostAgent {
  private hostId: string | null = null
  private capabilitiesHeld: string[] = []

  constructor(
    private readonly socket: HostSocket,
    private readonly name: string = machineName(),
    private readonly platform: string = platformName(),
    private readonly run: CommandRunner = runInShell,
    private readonly capabilities: readonly string[] = [SHELL_CAPABILITY],
  ) {
    this.socket.onMessage((frame) => {
      void this.receive(frame)
    })
  }

  /** Authenticate the socket and register this machine. */
  start(): void {
    this.socket.send(JSON.stringify({ type: 'auth' }))
    this.socket.send(
      JSON.stringify({
        type: 'register_host',
        name: this.name,
        platform: this.platform,
        capabilities: this.capabilities,
      }),
    )
  }

  /** The id the daemon knows this machine by, once it has acknowledged
   *  the registration. */
  registeredId(): string | null {
    return this.hostId
  }

  /** The capabilities that the daemon stored for this machine, from the
   *  last acknowledgement of the registration. */
  registeredCapabilities(): readonly string[] {
    return this.capabilitiesHeld
  }

  private async receive(frame: string): Promise<void> {
    let parsed: { type?: string; payload?: Record<string, unknown> }
    try {
      parsed = JSON.parse(frame)
    } catch {
      return
    }
    if (parsed.type === 'host.registered') {
      const id = parsed.payload?.host_id
      this.hostId = typeof id === 'string' ? id : null
      const held = parsed.payload?.capabilities
      this.capabilitiesHeld = Array.isArray(held)
        ? held.filter((capability): capability is string => typeof capability === 'string')
        : []
      return
    }
    if (parsed.type !== 'dispatch') return
    const dispatch = parsed.payload as unknown as HostDispatch
    if (
      typeof dispatch?.id !== 'string' ||
      typeof dispatch.command !== 'string' ||
      typeof dispatch.approved_by_rule !== 'boolean'
    ) {
      return
    }
    let result: HostResult
    try {
      result = await this.run(dispatch)
    } catch (error) {
      // The client failed to run it at all. The sprite reads that rather
      // than waiting for a result that is not coming.
      result = { exit_code: null, stdout: '', stderr: `${(error as Error).message}\n` }
    }
    this.socket.send(JSON.stringify({ type: 'result', id: dispatch.id, ...result }))
  }
}

/**
 * Keep this machine registered for as long as the client runs.
 *
 * The socket is the registration, so a socket that closes takes the
 * machine's presence with it and the link opens another one. A daemon
 * that restarts, a laptop that slept and a network that dropped are the
 * same case, and the person does nothing about any of them.
 *
 * A socket that closes with [`SESSION_ENDED`] lost its Session. The link
 * tells `sessionEnded`, and the next `open` must not use that Session.
 */
export class HostLink {
  private agent: HostAgent | null = null
  private socket: HostSocket | null = null
  private stopped = false
  private timer: ReturnType<typeof setTimeout> | null = null

  constructor(
    private readonly open: () => Promise<HostSocket>,
    private readonly retryMs = 3_000,
    private readonly run: CommandRunner = runInShell,
    private readonly sessionEnded: () => void = () => {},
    private readonly capabilities: readonly string[] = [SHELL_CAPABILITY],
  ) {}

  /** Open the socket and register, and keep doing so until [`stop`]. */
  start(): void {
    this.stopped = false
    void this.connect()
  }

  /** Stop and close the socket, which makes the machine absent at once. */
  stop(): void {
    this.stopped = true
    if (this.timer !== null) clearTimeout(this.timer)
    this.timer = null
    this.agent = null
    this.socket?.close()
    this.socket = null
  }

  /** The machine's id, once the daemon has acknowledged it. */
  registeredId(): string | null {
    return this.agent?.registeredId() ?? null
  }

  /** The machine's capabilities as the daemon acknowledged them, or none
   *  while no socket is registered. */
  registeredCapabilities(): readonly string[] {
    return this.agent?.registeredCapabilities() ?? []
  }

  /** Send one frame on the socket that is open now. It answers `false`
   *  when no socket is open, and the frame then goes nowhere. */
  send(frame: string): boolean {
    if (this.socket === null) return false
    this.socket.send(frame)
    return true
  }

  private async connect(): Promise<void> {
    if (this.stopped) return
    let socket: HostSocket
    try {
      socket = await this.open()
    } catch {
      this.retry()
      return
    }
    if (this.stopped) {
      socket.close()
      return
    }
    this.socket = socket
    socket.onClose((code) => {
      this.agent = null
      this.socket = null
      if (code === SESSION_ENDED) this.sessionEnded()
      this.retry()
    })
    this.agent = new HostAgent(socket, machineName(), platformName(), this.run, this.capabilities)
    this.agent.start()
  }

  private retry(): void {
    if (this.stopped || this.timer !== null) return
    this.timer = setTimeout(() => {
      this.timer = null
      void this.connect()
    }, this.retryMs)
  }
}

/**
 * A [`HostSocket`] over a real WebSocket to the daemon.
 *
 * The socket authenticates by the same Session cookie a REST call
 * carries, so the machine registers as the person the client signed in
 * as. The secret is a header of the handshake and never a query
 * parameter, because a URL is written to logs and a header is not.
 *
 * The socket carries every command the server dispatches, so it opens
 * only to a server the client trusts: over `wss://`, or over `ws://` on
 * loopback. A Host registers on no clear-text connection to another
 * machine.
 */
export async function openWebSocket(
  url: string,
  sessionSecret: string,
  cookieName = 'pagis_session',
): Promise<HostSocket> {
  const target = new URL('/api/v1/ws', url)
  if (!isTrustedServerOrigin(target)) {
    throw new Error(
      `the Host socket does not open to ${target.origin}: it opens only over https://, or over http:// on loopback`,
    )
  }
  target.protocol = target.protocol === 'https:' ? 'wss:' : 'ws:'
  const socket = new WebSocket(target, {
    headers: { cookie: `${cookieName}=${sessionSecret}` },
  } as unknown as string[])
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve(), { once: true })
    socket.addEventListener('error', () => reject(new Error('the host socket did not open')), {
      once: true,
    })
  })
  return {
    send: (frame) => socket.send(frame),
    onMessage: (listener) =>
      socket.addEventListener('message', (event) => {
        if (typeof event.data === 'string') listener(event.data)
      }),
    onClose: (listener) =>
      socket.addEventListener('close', (event) => listener(event.code), { once: true }),
    close: () => socket.close(),
  }
}
