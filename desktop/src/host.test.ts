// This client as the Host of its machine: the registration, the
// command it runs for a dispatch, and what it answers.

import { createHash } from 'node:crypto'
import { chmodSync, mkdtempSync, writeFileSync } from 'node:fs'
import http, { type IncomingHttpHeaders } from 'node:http'
import type { AddressInfo } from 'node:net'
import os from 'node:os'
import path from 'node:path'
import type { Duplex } from 'node:stream'

import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  EXIT_CAPABILITY,
  type HarnessFinder,
  harnessesOnPath,
  HostAgent,
  HostLink,
  type HostSocket,
  machineName,
  openWebSocket,
  platformName,
  runInShell,
  SHELL_CAPABILITY,
} from './host'

/** A socket a test drives: it keeps what the client sent, and hands the
 *  client whatever frame the test wants it to receive. */
class FakeSocket implements HostSocket {
  sent: Record<string, unknown>[] = []
  closed = false
  listener: ((frame: string) => void) | null = null
  private closeListener: ((code: number) => void) | null = null

  send(frame: string): void {
    this.sent.push(JSON.parse(frame))
  }

  onMessage(listener: (frame: string) => void): void {
    this.listener = listener
  }

  onClose(listener: (code: number) => void): void {
    this.closeListener = listener
  }

  /** The connection drops, with no close frame. */
  close(): void {
    this.closed = true
    this.closeListener?.(1006)
  }

  /** The daemon says something. */
  receive(frame: Record<string, unknown>): void {
    this.listener?.(JSON.stringify(frame))
  }

  /** Wait for the client's answer to a dispatch. */
  async settled(): Promise<Record<string, unknown>> {
    for (let attempt = 0; attempt < 100; attempt += 1) {
      const result = this.sent.find((frame) => frame.type === 'result')
      if (result !== undefined) return result
      await new Promise((resolve) => setTimeout(resolve, 10))
    }
    throw new Error(`the client answered nothing: ${JSON.stringify(this.sent)}`)
  }
}

function dispatch(command: string, id = 'd-1', approvedByRule = false) {
  return {
    type: 'dispatch',
    payload: { id, command, timeout_ms: 5_000, approved_by_rule: approvedByRule },
  }
}

describe('the host registration', () => {
  it('authenticates and registers this machine as one that runs commands', () => {
    const socket = new FakeSocket()

    new HostAgent(socket, 'Air', 'macos').start()

    expect(socket.sent).toEqual([
      { type: 'auth' },
      {
        type: 'register_host',
        name: 'Air',
        platform: 'macos',
        capabilities: ['shell'],
      },
    ])
  })

  /** A client connected to a server declares the exit too, so that its
   *  Person can choose the machine as their Home Exit. */
  it('declares the capabilities it is given', () => {
    const socket = new FakeSocket()

    new HostAgent(socket, 'Air', 'macos', runInShell, [SHELL_CAPABILITY, EXIT_CAPABILITY]).start()

    expect(socket.sent[1]).toEqual({
      type: 'register_host',
      name: 'Air',
      platform: 'macos',
      capabilities: ['shell', 'exit'],
    })
  })

  it('keeps the id the daemon gives the machine', () => {
    const socket = new FakeSocket()
    const agent = new HostAgent(socket, 'Air', 'macos')
    agent.start()

    socket.receive({ type: 'host.registered', payload: { host_id: 'h-1' } })

    expect(agent.registeredId()).toBe('h-1')
  })

  it('keeps the capabilities of the last registration that the daemon acknowledged', () => {
    const socket = new FakeSocket()
    const agent = new HostAgent(socket, 'Air', 'macos')
    agent.start()
    expect(agent.registeredCapabilities()).toEqual([])

    socket.receive({ type: 'host.registered', payload: { host_id: 'h-1', capabilities: ['shell'] } })
    expect(agent.registeredCapabilities()).toEqual(['shell'])
    socket.receive({
      type: 'host.registered',
      payload: { host_id: 'h-1', capabilities: ['shell', 'harness:claude', 7] },
    })

    // A value that is not a string is no capability.
    expect(agent.registeredCapabilities()).toEqual(['shell', 'harness:claude'])
  })

  /** The name is what the person calls the computer, without the network
   *  suffix a Mac carries. */
  it('names the machine as the person knows it', () => {
    expect(machineName('Airs-MacBook.local')).toBe('Airs-MacBook')
    expect(machineName('workstation')).toBe('workstation')
    expect(platformName('darwin')).toBe('macos')
    expect(platformName('win32')).toBe('windows')
    expect(platformName('linux')).toBe('linux')
  })
})

/** The answer to a registration names the Harness Catalog. The client
 *  looks for the programs of each harness on the PATH of the login shell
 *  and registers again with a `harness:<id>` capability for each harness
 *  it found. */
describe('the Coding Harnesses of the machine', () => {
  const CATALOG = [
    { id: 'claude', launchers: ['npx'] },
    { id: 'opencode', launchers: ['opencode'] },
    { id: 'pi', launchers: ['npx', 'pi'] },
  ]

  /** A directory on the PATH that holds an executable for each name. */
  function pathWith(...programs: string[]): string {
    const directory = mkdtempSync(path.join(os.tmpdir(), 'pagis-path-'))
    for (const program of programs) {
      const file = path.join(directory, program)
      writeFileSync(file, '#!/bin/sh\n')
      chmodSync(file, 0o755)
    }
    return directory
  }

  function finderOn(pathValue: string): HarnessFinder {
    return (catalog) => harnessesOnPath(catalog, async () => ({ PATH: pathValue }))
  }

  function registered(capabilities: string[], harnesses?: unknown) {
    return {
      type: 'host.registered',
      payload: { host_id: 'h-1', platform: 'macos', capabilities, harnesses },
    }
  }

  /** Wait until the client has handled every frame it received. */
  async function settle(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 20))
  }

  it('registers again with each harness whose launchers are on the PATH, and not a third time', async () => {
    const socket = new FakeSocket()
    const agent = new HostAgent(socket, 'Air', 'macos', runInShell, [SHELL_CAPABILITY, EXIT_CAPABILITY], finderOn(pathWith('npx')))
    agent.start()

    socket.receive(registered(['shell', 'exit'], CATALOG))
    await vi.waitFor(() => expect(socket.sent).toHaveLength(3))

    expect(socket.sent[2]).toEqual({
      type: 'register_host',
      name: 'Air',
      platform: 'macos',
      capabilities: ['shell', 'exit', 'harness:claude'],
    })

    socket.receive(registered(['shell', 'exit', 'harness:claude'], CATALOG))
    await settle()

    expect(socket.sent).toHaveLength(3)
    expect(agent.registeredCapabilities()).toEqual(['shell', 'exit', 'harness:claude'])
  })

  /** pi needs its own program beside `npx`. */
  it('adds no capability for a harness whose launcher is not on the PATH', async () => {
    const socket = new FakeSocket()
    new HostAgent(socket, 'Air', 'macos', runInShell, [SHELL_CAPABILITY], finderOn(pathWith('pi'))).start()

    socket.receive(registered(['shell'], CATALOG))
    await settle()

    expect(socket.sent).toHaveLength(2)
  })

  it('declares no harness and writes the message to the log when the login shell fails', async () => {
    const log = vi.spyOn(console, 'error').mockImplementation(() => {})
    const socket = new FakeSocket()
    new HostAgent(socket, 'Air', 'macos', runInShell, [SHELL_CAPABILITY], (catalog) =>
      harnessesOnPath(catalog, async () => {
        throw new Error('the login shell /bin/zsh exited with code 1')
      }),
    ).start()

    socket.receive(registered(['shell'], CATALOG))
    await settle()

    expect(socket.sent).toHaveLength(2)
    expect(log).toHaveBeenCalledWith(expect.stringContaining('the login shell /bin/zsh exited with code 1'))
    log.mockRestore()
  })

  it('looks for nothing when the answer names no harnesses', async () => {
    const socket = new FakeSocket()
    const find = vi.fn(async () => ['claude'])
    new HostAgent(socket, 'Air', 'macos', runInShell, [SHELL_CAPABILITY], find).start()

    socket.receive(registered(['shell']))
    await settle()

    expect(find).not.toHaveBeenCalled()
    expect(socket.sent).toHaveLength(2)
  })
})

describe('a dispatched command', () => {
  it('runs on this machine and answers with what it did', async () => {
    const socket = new FakeSocket()
    new HostAgent(socket, 'Air', 'macos', async (command) => ({
      exit_code: 0,
      stdout: `ran ${command.command}\n`,
      stderr: '',
    })).start()

    socket.receive(dispatch('git status'))

    expect(await socket.settled()).toEqual({
      type: 'result',
      id: 'd-1',
      exit_code: 0,
      stdout: 'ran git status\n',
      stderr: '',
    })
  })

  /** The runner reads the shell to use from the dispatch. */
  it('hands the runner whether an Allow Rule approved the command', async () => {
    const socket = new FakeSocket()
    const run = vi.fn(async () => ({ exit_code: 0, stdout: '', stderr: '' }))
    new HostAgent(socket, 'Air', 'macos', run).start()

    socket.receive(dispatch('git status', 'd-1', true))
    await socket.settled()

    expect(run).toHaveBeenCalledWith({
      id: 'd-1',
      command: 'git status',
      timeout_ms: 5_000,
      approved_by_rule: true,
    })
  })

  /** A command that fails is an answer the person reads, not a silence
   *  that leaves the sprite waiting out the daemon's deadline. */
  it('answers a failed command rather than saying nothing', async () => {
    const socket = new FakeSocket()
    new HostAgent(socket, 'Air', 'macos', async () => {
      throw new Error('no shell on this machine')
    }).start()

    socket.receive(dispatch('git status'))

    const result = await socket.settled()
    expect(result.exit_code).toBe(null)
    expect(result.stderr).toContain('no shell on this machine')
  })

  it('ignores a frame that is not a dispatch and one that names no command', async () => {
    const socket = new FakeSocket()
    const run = vi.fn()
    new HostAgent(socket, 'Air', 'macos', run).start()

    socket.receive({ type: 'run.state_changed', payload: { to: 'completed' } })
    socket.receive({ type: 'dispatch', payload: { id: 'd-1' } })
    socket.receive({ type: 'dispatch', payload: { command: 'id' } })
    // A dispatch that does not say how it was approved does not say
    // which shell it runs in.
    socket.receive({ type: 'dispatch', payload: { id: 'd-1', command: 'id', timeout_ms: 5_000 } })
    socket.listener?.('not json')

    expect(run).not.toHaveBeenCalled()
    expect(socket.sent.some((frame) => frame.type === 'result')).toBe(false)
  })
})

describe('the shell of the OS user', () => {
  it('runs the command and reports its output and exit code', async () => {
    const ok = await runInShell({
      id: 'd-1',
      command: 'echo hello',
      timeout_ms: 5_000,
      approved_by_rule: false,
    })

    expect(ok.exit_code).toBe(0)
    expect(ok.stdout.trim()).toBe('hello')
    expect(ok.stderr).toBe('')
  })

  it('reports a non-zero exit as the result and not as a failure', async () => {
    const failed = await runInShell({
      id: 'd-2',
      command: 'exit 3',
      timeout_ms: 5_000,
      approved_by_rule: false,
    })

    expect(failed.exit_code).toBe(3)
  })

  it('stops a command at the deadline and says so', async () => {
    const stopped = await runInShell({
      id: 'd-3',
      command: 'sleep 5',
      timeout_ms: 100,
      approved_by_rule: false,
    })

    expect(stopped.stderr).toContain('stopped at its deadline')
  })
})

/** A stand-in for the person's login shell. It says that it ran and
 *  what it received, so a test can tell which shell ran the command
 *  whatever the shell of the machine that runs the test is. */
function personShell(): string {
  const file = path.join(mkdtempSync(path.join(os.tmpdir(), 'pagis-shell-')), 'person-shell')
  writeFileSync(file, '#!/bin/sh\nprintf \'person shell: %s\\n\' "$2"\n')
  chmodSync(file, 0o755)
  return file
}

describe('the shell a dispatch runs in', () => {
  afterEach(() => {
    vi.unstubAllEnvs()
  })

  /** The daemon checked the command as POSIX sh, so it runs in the
   *  dialect that was checked and not in the person's shell. */
  it('runs a command that an Allow Rule approved under /bin/sh and not under $SHELL', async () => {
    vi.stubEnv('SHELL', personShell())

    const result = await runInShell({
      id: 'd-1',
      command: 'echo "$0"',
      timeout_ms: 5_000,
      approved_by_rule: true,
    })

    expect(result.stdout).toBe('/bin/sh\n')
  })

  it('runs a command that the person approved on its card in their $SHELL', async () => {
    vi.stubEnv('SHELL', personShell())

    const result = await runInShell({
      id: 'd-1',
      command: 'echo "$0"',
      timeout_ms: 5_000,
      approved_by_rule: false,
    })

    expect(result.stdout).toBe('person shell: echo "$0"\n')
  })
})

/** A stand-in for the global WebSocket. It records the target of each
 *  socket and opens at once. */
class RecordedWebSocket extends EventTarget {
  static opened: string[] = []

  constructor(target: URL | string) {
    super()
    RecordedWebSocket.opened.push(String(target))
    queueMicrotask(() => this.dispatchEvent(new Event('open')))
  }

  send(): void {}
  close(): void {}
}

/** The Host socket carries every command the server dispatches, so it
 *  opens only where the client trusts the server: over TLS, or on
 *  loopback. */
describe('the Host socket', () => {
  afterEach(() => {
    RecordedWebSocket.opened = []
    vi.unstubAllGlobals()
  })

  it('opens over wss:// to a server over TLS, and over ws:// on loopback', async () => {
    vi.stubGlobal('WebSocket', RecordedWebSocket)

    await openWebSocket('https://pagis.example.com/', 'session')
    await openWebSocket('http://127.0.0.1:4400/', 'session')
    await openWebSocket('http://localhost:4400/', 'session')

    expect(RecordedWebSocket.opened).toEqual([
      'wss://pagis.example.com/api/v1/ws',
      'ws://127.0.0.1:4400/api/v1/ws',
      'ws://localhost:4400/api/v1/ws',
    ])
  })

  it('opens no ws:// socket to another computer', async () => {
    vi.stubGlobal('WebSocket', RecordedWebSocket)

    await expect(openWebSocket('http://192.168.1.10:4400/', 'session')).rejects.toThrow(/https:\/\//)
    await expect(openWebSocket('http://pagis.example.com/', 'session')).rejects.toThrow(/https:\/\//)

    expect(RecordedWebSocket.opened).toEqual([])
  })

  // The daemon refuses a socket upgrade that a browser page on another
  // origin starts, and a browser sends `Origin` or `Sec-Fetch-Site` on
  // every handshake. The Client App is not a browser page: it sends
  // neither, so the daemon passes its socket to the Session check. The
  // daemon test `the_host_socket_of_the_client_app_registers_a_host`
  // sends these headers.
  it('sends the Session cookie and no Origin or Sec-Fetch-Site in its handshake', async () => {
    const headers = await handshakeHeaders((url) => openWebSocket(url, 'secret'))

    expect(headers.cookie).toBe('pagis_session=secret')
    expect(headers).not.toHaveProperty('origin')
    expect(headers).not.toHaveProperty('sec-fetch-site')
  })
})

/** Open a socket with `open` to a loopback server that accepts the
 *  handshake, and answer the headers of that handshake. */
async function handshakeHeaders(
  open: (url: string) => Promise<HostSocket>,
): Promise<IncomingHttpHeaders> {
  const handshake: { headers?: IncomingHttpHeaders; socket?: Duplex } = {}
  const server = http.createServer()
  server.on('upgrade', (request, socket) => {
    handshake.headers = request.headers
    handshake.socket = socket
    socket.on('error', () => {})
    const accept = createHash('sha1')
      .update(`${request.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
      .digest('base64')
    socket.write(
      [
        'HTTP/1.1 101 Switching Protocols',
        'Upgrade: websocket',
        'Connection: Upgrade',
        `Sec-WebSocket-Accept: ${accept}`,
        '',
        '',
      ].join('\r\n'),
    )
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address() as AddressInfo
  try {
    const socket = await open(`http://127.0.0.1:${port}/`)
    socket.close()
  } finally {
    handshake.socket?.destroy()
    await new Promise((resolve) => server.close(resolve))
  }
  if (handshake.headers === undefined) throw new Error('the client sent no handshake')
  return handshake.headers
}

describe('the link that keeps the machine registered', () => {
  it('registers again after the socket closes', async () => {
    const sockets: FakeSocket[] = []
    const link = new HostLink(
      async () => {
        const socket = new FakeSocket()
        sockets.push(socket)
        return socket
      },
      1,
    )

    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))
    sockets[0].close()
    await vi.waitFor(() => expect(sockets).toHaveLength(2))
    expect(sockets[1].sent[1]).toMatchObject({ type: 'register_host' })

    link.stop()
    expect(sockets[1].closed).toBe(true)
  })

  it('registers each socket with the capabilities it is given', async () => {
    const sockets: FakeSocket[] = []
    const link = new HostLink(
      async () => {
        const socket = new FakeSocket()
        sockets.push(socket)
        return socket
      },
      1,
      runInShell,
      () => {},
      [SHELL_CAPABILITY, EXIT_CAPABILITY],
    )

    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))
    sockets[0].close()
    await vi.waitFor(() => expect(sockets).toHaveLength(2))
    link.stop()

    for (const socket of sockets) {
      expect(socket.sent[1]).toMatchObject({ type: 'register_host', capabilities: ['shell', 'exit'] })
    }
  })

  it('keeps trying while the daemon refuses the socket', async () => {
    let attempts = 0
    const link = new HostLink(async () => {
      attempts += 1
      if (attempts < 3) throw new Error('connection refused')
      return new FakeSocket()
    }, 1)

    link.start()
    await vi.waitFor(() => expect(attempts).toBe(3))

    link.stop()
  })

  it('gives the capabilities of its registration, and sends a frame on the socket that is open', async () => {
    const sockets: FakeSocket[] = []
    const link = new HostLink(
      async () => {
        const socket = new FakeSocket()
        sockets.push(socket)
        return socket
      },
      60_000,
    )
    expect(link.send(JSON.stringify({ type: 'ping' }))).toBe(false)

    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))
    sockets[0].receive({ type: 'host.registered', payload: { host_id: 'h-1', capabilities: ['harness:claude'] } })

    expect(link.registeredCapabilities()).toEqual(['harness:claude'])
    expect(link.send(JSON.stringify({ type: 'session_exit', session_id: 's-1' }))).toBe(true)
    expect(sockets[0].sent.at(-1)).toEqual({ type: 'session_exit', session_id: 's-1' })

    // The socket dropped: no registration and no socket until the next one.
    sockets[0].close()
    expect(link.registeredCapabilities()).toEqual([])
    expect(link.send(JSON.stringify({ type: 'ping' }))).toBe(false)
    link.stop()
  })

  it('opens nothing more once it is stopped', async () => {
    let attempts = 0
    const link = new HostLink(async () => {
      attempts += 1
      throw new Error('connection refused')
    }, 1)

    link.start()
    await vi.waitFor(() => expect(attempts).toBeGreaterThan(0))
    link.stop()
    const settled = attempts

    await new Promise((resolve) => setTimeout(resolve, 20))

    expect(attempts).toBe(settled)
  })
})
