// The Home Exit side of this client: the check of an address, the
// preamble of a stream, the connection that a stream carries, and the link
// that keeps the exit socket open. The last case runs the interop harness
// of the daemon's test under plain node. `byteSocket.test.ts` tests the
// socket itself.

import { spawn } from 'node:child_process'
import net, { type AddressInfo, type Socket } from 'node:net'
import type { NetworkInterfaceInfo } from 'node:os'
import path from 'node:path'
import { type Duplex, duplexPair } from 'node:stream'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { daemonServer } from '../test/websocketDaemon'
import { GO_AWAY, openStream, streamBytes, WINDOW_UPDATE, yamuxFrames } from '../test/yamuxFrames'
import type { ByteSocket } from './byteSocket'
import {
  carry,
  type Connect,
  connectTcp,
  type Destination,
  type Dial,
  ExitLink,
  ExitTraffic,
  homeDial,
  type Interfaces,
  isRefusedAddress,
  parseDestination,
  RefusedDestination,
} from './exit'

describe('the address check', () => {
  // Loopback, private, link-local, carrier-grade NAT, multicast,
  // unspecified and broadcast addresses are on the home network or on
  // this machine.
  const refused = [
    ['0.0.0.0', 'IPv4 unspecified'],
    ['0.1.2.3', 'IPv4 this network'],
    ['10.0.0.1', 'IPv4 private'],
    ['10.255.255.255', 'IPv4 private'],
    ['100.64.0.1', 'carrier-grade NAT'],
    ['100.127.255.254', 'carrier-grade NAT'],
    ['127.0.0.1', 'IPv4 loopback'],
    ['127.255.255.254', 'IPv4 loopback'],
    ['169.254.169.254', 'IPv4 link-local'],
    ['172.16.0.1', 'IPv4 private'],
    ['172.31.255.255', 'IPv4 private'],
    ['192.168.1.10', 'IPv4 private'],
    ['224.0.0.251', 'IPv4 multicast'],
    ['239.255.255.250', 'IPv4 multicast'],
    ['255.255.255.255', 'IPv4 broadcast'],
    ['::', 'IPv6 unspecified'],
    ['::1', 'IPv6 loopback'],
    ['fc00::1', 'IPv6 unique local'],
    ['fdff:ffff::1', 'IPv6 unique local'],
    ['fe80::1', 'IPv6 link-local'],
    ['febf::1', 'IPv6 link-local'],
    ['ff02::fb', 'IPv6 multicast'],
    ['::ffff:127.0.0.1', 'IPv4-mapped loopback'],
    ['::ffff:7f00:1', 'IPv4-mapped loopback in hex'],
    ['::ffff:192.168.1.10', 'IPv4-mapped private'],
    ['::ffff:a9fe:a9fe', 'IPv4-mapped link-local in hex'],
    ['64:ff9b::127.0.0.1', 'NAT64 of loopback'],
    ['64:ff9b::7f00:1', 'NAT64 of loopback in hex'],
    ['64:ff9b::c0a8:10a', 'NAT64 of a private address'],
    ['64:ff9b::', 'NAT64 of the unspecified address'],
    ['fe80::1%en0', 'IPv6 link-local with a zone'],
    ['example.com', 'not an address'],
    ['', 'not an address'],
  ]

  const allowed = [
    ['8.8.8.8', 'public IPv4'],
    ['11.0.0.1', 'public IPv4 after 10/8'],
    ['100.63.255.255', 'public IPv4 before 100.64/10'],
    ['100.128.0.0', 'public IPv4 after 100.64/10'],
    ['169.253.255.255', 'public IPv4 before 169.254/16'],
    ['172.15.255.255', 'public IPv4 before 172.16/12'],
    ['172.32.0.0', 'public IPv4 after 172.16/12'],
    ['192.169.0.1', 'public IPv4 after 192.168/16'],
    ['223.255.255.255', 'public IPv4 before 224/4'],
    ['2001:4860:4860::8888', 'public IPv6'],
    ['2606:4700:4700::1111', 'public IPv6'],
    ['fbff::1', 'public IPv6 before fc00::/7'],
    ['::ffff:8.8.8.8', 'IPv4-mapped public'],
    ['64:ff9b::808:808', 'NAT64 of a public address'],
    ['64:ff9b::8.8.8.8', 'NAT64 of a public address, dotted'],
  ]

  it.each(refused)('refuses %s (%s)', (address) => {
    expect(isRefusedAddress(address)).toBe(true)
  })

  it.each(allowed)('allows %s (%s)', (address) => {
    expect(isRefusedAddress(address)).toBe(false)
  })
})

/** One address of an interface, as `os.networkInterfaces` gives it. */
function address(cidr: string): NetworkInterfaceInfo {
  const [value] = cidr.split('/')
  return net.isIPv4(value)
    ? { address: value, netmask: '', family: 'IPv4', mac: '00:00:00:00:00:00', internal: false, cidr }
    : { address: value, netmask: '', family: 'IPv6', mac: '00:00:00:00:00:00', internal: false, cidr, scopeid: 0 }
}

describe('the networks of this machine', () => {
  // A home network with global IPv6 addresses and a public IPv4 LAN, and
  // a tunnel whose address is the machine's own public address.
  const home: Interfaces = () => ({
    lo0: [address('127.0.0.1/8'), address('::1/128')],
    en0: [address('2001:db8:1:2::10/64'), address('203.0.113.9/24')],
    utun0: [address('198.51.100.20/32')],
  })

  it.each([
    ["the machine's own global IPv6 address", '2001:db8:1:2::10'],
    ['a printer in the home /64', '2001:db8:1:2::99'],
    ['an address of the public IPv4 LAN', '203.0.113.77'],
    ['the IPv4-mapped form of an address of that LAN', '::ffff:203.0.113.77'],
    ["the machine's own public IPv4 address", '198.51.100.20'],
  ])('refuses %s, which no fixed range holds', async (_what, local) => {
    expect(isRefusedAddress(local)).toBe(false)
    const connect = vi.fn(async () => ({}) as Socket)
    const dial = homeDial(async () => [{ address: local }], connect, home)

    await expect(dial({ host: 'nas.example.com', port: 443 }, new AbortController().signal)).rejects.toBeInstanceOf(
      RefusedDestination,
    )
    expect(connect).not.toHaveBeenCalled()
  })

  it('connects to an address outside every local subnet', async () => {
    const connect = vi.fn<Connect>(async () => ({}) as Socket)
    const signal = new AbortController().signal

    for (const outside of ['2001:db8:1:3::1', '203.0.114.1', '198.51.100.21']) {
      await homeDial(async () => [{ address: outside }], connect, home)({ host: 'example.com', port: 443 }, signal)
    }

    expect(connect.mock.calls.map(([host]) => host)).toEqual(['2001:db8:1:3::1', '203.0.114.1', '198.51.100.21'])
  })

  it('skips a local address and connects to the next one that passes', async () => {
    const connect = vi.fn(async () => ({}) as Socket)
    const signal = new AbortController().signal

    await homeDial(async () => [{ address: '2001:db8:1:2::99' }, { address: '2001:db8:9::1' }], connect, home)(
      { host: 'example.com', port: 443 },
      signal,
    )

    expect(connect).toHaveBeenCalledWith('2001:db8:9::1', 443, signal)
  })

  it('reads the interfaces at each dial, as a laptop moves from one network to another', async () => {
    const connect = vi.fn(async () => ({}) as Socket)
    const signal = new AbortController().signal
    let network = '2001:db8:1:2::10/64'
    const interfaces = vi.fn<Interfaces>(() => ({ en0: [address(network)] }))
    const dial = homeDial(async () => [{ address: '2001:db8:1:2::99' }], connect, interfaces)

    await expect(dial({ host: 'example.com', port: 443 }, signal)).rejects.toBeInstanceOf(RefusedDestination)
    network = '2001:db8:7:7::10/64'
    await dial({ host: 'example.com', port: 443 }, signal)

    expect(interfaces).toHaveBeenCalledTimes(2)
    expect(connect).toHaveBeenCalledWith('2001:db8:1:2::99', 443, signal)
  })
})

describe('the destination of a stream', () => {
  it('reads a name, an IPv4 address and a bracketed IPv6 address with their ports', () => {
    expect(parseDestination('example.com:443')).toEqual({ host: 'example.com', port: 443 })
    expect(parseDestination('203.0.113.7:80')).toEqual({ host: '203.0.113.7', port: 80 })
    expect(parseDestination('[2001:db8::1]:443')).toEqual({ host: '2001:db8::1', port: 443 })
  })

  it.each([
    ['example.com', 'no port'],
    ['example.com:', 'an empty port'],
    [':443', 'no host'],
    ['example.com:0', 'port 0'],
    ['example.com:65536', 'a port past 65535'],
    ['example.com:44a', 'a port that is not a number'],
    ['2001:db8::1:443', 'an IPv6 address with no brackets'],
    ['[example.com]:443', 'a name in brackets'],
    ['[2001:db8::1]', 'a bracketed address with no port'],
    ['exa mple.com:443', 'a space in the name'],
    ['example.com:443\r', 'a carriage return'],
  ])('refuses %j (%s)', (line) => {
    expect(() => parseDestination(line)).toThrow()
  })
})

/** A site on loopback. It says hello to each connection, and keeps what
 *  each connection sent. */
async function site(): Promise<{
  port: number
  connections: Socket[]
  received: () => string
  close: () => Promise<void>
}> {
  const connections: Socket[] = []
  const chunks: Buffer[] = []
  const server = net.createServer({ allowHalfOpen: true }, (socket) => {
    connections.push(socket)
    socket.on('error', () => {})
    socket.on('data', (chunk: Buffer) => chunks.push(chunk))
    socket.write('hello from the site')
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  return {
    port: (server.address() as AddressInfo).port,
    connections,
    received: () => Buffer.concat(chunks).toString(),
    close: async () => {
      for (const socket of connections) socket.destroy()
      await new Promise((resolve) => server.close(resolve))
    },
  }
}

/** A port on loopback where nothing listens. */
async function closedPort(): Promise<number> {
  const server = net.createServer()
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address() as AddressInfo
  await new Promise((resolve) => server.close(resolve))
  return port
}

/** A dial that connects every stream to a port on loopback, which the
 *  real check refuses. It keeps each destination it was asked for. */
function loopbackDial(port: number, asked: Destination[] = []): Dial {
  return (destination, signal) => {
    asked.push(destination)
    return connectTcp('127.0.0.1', port, signal)
  }
}

/** A stream as the daemon opens it: `carry` gets one end, and the test
 *  is the daemon at the other end. */
function stream(dial: Dial, traffic?: ExitTraffic): { daemon: Duplex; answer: () => string } {
  const [client, daemon] = duplexPair()
  const chunks: Buffer[] = []
  daemon.on('data', (chunk: Buffer) => chunks.push(chunk))
  daemon.on('error', () => {})
  void carry(client, dial, traffic)
  return { daemon, answer: () => Buffer.concat(chunks).toString() }
}

/** Wait until the readable side of `side` ended. */
function ended(side: Duplex): Promise<void> {
  return new Promise((resolve) => {
    if (side.readableEnded) resolve()
    else side.once('end', () => resolve())
  })
}

/** Wait until the site's end of a connection saw it end or close. */
function released(side: Duplex): Promise<void> {
  return new Promise((resolve) => {
    if (side.readableEnded || side.destroyed) resolve()
    side.once('end', () => resolve())
    side.once('close', () => resolve())
  })
}

function closed(side: Duplex): Promise<void> {
  return new Promise((resolve) => {
    if (side.destroyed) resolve()
    else side.once('close', () => resolve())
  })
}

describe('a stream that carries one connection', () => {
  const sites: Array<{ close: () => Promise<void> }> = []
  afterEach(async () => {
    vi.useRealTimers()
    for (const opened of sites.splice(0)) await opened.close()
  })

  async function openSite() {
    const opened = await site()
    sites.push(opened)
    return opened
  }

  it('answers ok, then carries the bytes both ways', async () => {
    const target = await openSite()
    const asked: Destination[] = []
    const { daemon, answer } = stream(loopbackDial(target.port, asked))

    // The daemon can send the first bytes of the connection with the
    // preamble.
    daemon.write('example.com:443\nGET / HTTP/1.1\r\n')

    await vi.waitFor(() => expect(answer()).toBe('ok\nhello from the site'))
    await vi.waitFor(() => expect(target.received()).toBe('GET / HTTP/1.1\r\n'))
    expect(asked).toEqual([{ host: 'example.com', port: 443 }])
    daemon.destroy()
  })

  it('counts the connection while it is open, and the bytes that it copies both ways', async () => {
    const target = await openSite()
    const traffic = new ExitTraffic()
    const { daemon, answer } = stream(loopbackDial(target.port), traffic)

    daemon.write('example.com:443\nGET / HTTP/1.1\r\n')

    await vi.waitFor(() => expect(answer()).toBe('ok\nhello from the site'))
    await vi.waitFor(() => expect(target.received()).toBe('GET / HTTP/1.1\r\n'))
    // The request one way and the greeting the other, and no preamble or
    // status line: those are the protocol and not the connection.
    await vi.waitFor(() => expect(traffic.bytes).toBe(16 + 19))
    expect(traffic.connections).toBe(1)

    daemon.destroy(new Error('the daemon reset the stream'))
    await vi.waitFor(() => expect(traffic.connections).toBe(0))
    expect(traffic.bytes).toBe(35)
  })

  it('counts no connection that it refused or that failed', async () => {
    const traffic = new ExitTraffic()
    const refused = stream(
      homeDial(async () => [{ address: '192.168.1.10' }], vi.fn()),
      traffic,
    )
    refused.daemon.write('printer.home.arpa:631\n')
    await ended(refused.daemon)
    const failed = stream(loopbackDial(await closedPort()), traffic)
    failed.daemon.write('example.com:443\n')
    await ended(failed.daemon)

    expect(traffic.connections).toBe(0)
    expect(traffic.bytes).toBe(0)
  })

  it('reads a preamble that arrives in parts', async () => {
    const target = await openSite()
    const asked: Destination[] = []
    const { daemon, answer } = stream(loopbackDial(target.port, asked))

    daemon.write('[2001:db8')
    await new Promise((resolve) => setTimeout(resolve, 5))
    daemon.write('::1]:443')
    await new Promise((resolve) => setTimeout(resolve, 5))
    daemon.write('\n')

    await vi.waitFor(() => expect(answer()).toBe('ok\nhello from the site'))
    expect(asked).toEqual([{ host: '2001:db8::1', port: 443 }])
    daemon.destroy()
  })

  it('half-closes the socket on a FIN of the stream, and ends the stream when the site ends', async () => {
    const target = await openSite()
    const { daemon, answer } = stream(loopbackDial(target.port))
    daemon.write('example.com:443\nrequest')
    await vi.waitFor(() => expect(target.connections).toHaveLength(1))
    const [socket] = target.connections

    daemon.end()
    await ended(socket)
    expect(target.received()).toBe('request')

    // The other half is still open: the site answers, then closes.
    socket.end('response')
    await ended(daemon)
    expect(answer()).toBe('ok\nhello from the siteresponse')
  })

  it('destroys the socket when the stream is reset, and the stream when the socket is reset', async () => {
    const target = await openSite()
    const first = stream(loopbackDial(target.port))
    first.daemon.write('example.com:443\n')
    await vi.waitFor(() => expect(target.connections).toHaveLength(1))

    first.daemon.destroy(new Error('the daemon reset the stream'))
    await released(target.connections[0])

    const second = stream(loopbackDial(target.port))
    second.daemon.write('example.com:443\n')
    await vi.waitFor(() => expect(target.connections).toHaveLength(2))
    await vi.waitFor(() => expect(second.answer()).toContain('ok\n'))

    target.connections[1].resetAndDestroy()
    await closed(second.daemon)
  })

  it('refuses a name whose every address is on a local network, and ends the stream', async () => {
    const connect = vi.fn()
    const dial = homeDial(async () => [{ address: '192.168.1.10' }, { address: '::1' }], connect)
    const { daemon, answer } = stream(dial)

    daemon.write('printer.home.arpa:631\n')
    await ended(daemon)

    expect(answer()).toMatch(/^refused [ -~]+\n$/)
    expect(answer()).toContain('printer.home.arpa')
    expect(connect).not.toHaveBeenCalled()
  })

  it('connects to the first address that passes the check and to no other', async () => {
    const connect = vi.fn(async () => ({}) as Socket)
    const dial = homeDial(
      async () => [{ address: '10.0.0.7' }, { address: '203.0.113.7' }, { address: '203.0.113.8' }],
      connect,
    )
    const signal = new AbortController().signal

    await dial({ host: 'example.com', port: 443 }, signal)

    expect(connect).toHaveBeenCalledTimes(1)
    expect(connect).toHaveBeenCalledWith('203.0.113.7', 443, signal)
  })

  it('resolves a literal address to itself', async () => {
    const connect = vi.fn(async () => ({}) as Socket)
    const signal = new AbortController().signal

    await homeDial(undefined, connect)({ host: '2001:db8::1', port: 443 }, signal)
    await expect(homeDial(undefined, connect)({ host: '127.0.0.1', port: 443 }, signal)).rejects.toThrow()

    expect(connect).toHaveBeenCalledTimes(1)
    expect(connect).toHaveBeenCalledWith('2001:db8::1', 443, signal)
  })

  it('answers failed for a name that does not resolve', async () => {
    const dial = homeDial(async () => {
      throw new Error('getaddrinfo ENOTFOUND nowhere.invalid')
    })
    const { daemon, answer } = stream(dial)

    daemon.write('nowhere.invalid:443\n')
    await ended(daemon)

    expect(answer()).toBe('failed getaddrinfo ENOTFOUND nowhere.invalid\n')
  })

  it('answers failed for a port where nothing listens', async () => {
    const { daemon, answer } = stream(loopbackDial(await closedPort()))

    daemon.write('example.com:443\n')
    await ended(daemon)

    expect(answer()).toMatch(/^failed .*ECONNREFUSED.*\n$/)
  })

  it('answers failed for a malformed preamble', async () => {
    const dial = vi.fn()
    const { daemon, answer } = stream(dial)

    daemon.write('example.com\n')
    await ended(daemon)

    expect(answer()).toMatch(/^failed [ -~]+\n$/)
    expect(dial).not.toHaveBeenCalled()
  })

  it('takes a preamble of 262 bytes and answers failed for a longer one', async () => {
    const target = await openSite()
    const asked: Destination[] = []
    const longest = `${'a'.repeat(257)}:443\n`
    expect(Buffer.byteLength(longest)).toBe(262)
    const fits = stream(loopbackDial(target.port, asked))
    fits.daemon.write(longest)
    await vi.waitFor(() => expect(fits.answer()).toContain('ok\n'))
    fits.daemon.destroy()

    const dial = vi.fn()
    const tooLong = stream(dial)
    tooLong.daemon.write(`${'a'.repeat(258)}:443`)
    await ended(tooLong.daemon)

    expect(tooLong.answer()).toMatch(/^failed [ -~]+\n$/)
    expect(dial).not.toHaveBeenCalled()
  })

  it('answers failed when the stream ends before its preamble', async () => {
    const dial = vi.fn()
    const { daemon, answer } = stream(dial)

    daemon.end('example.com:443')
    await ended(daemon)

    expect(answer()).toMatch(/^failed [ -~]+\n$/)
    expect(dial).not.toHaveBeenCalled()
  })

  it('destroys a stream whose preamble does not arrive in 10 s', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const dial = vi.fn()
    const { daemon, answer } = stream(dial)
    daemon.write('example.com:44')

    await vi.advanceTimersByTimeAsync(9_900)
    expect(daemon.destroyed).toBe(false)
    await vi.advanceTimersByTimeAsync(100)

    await closed(daemon)
    expect(answer()).toBe('')
    expect(dial).not.toHaveBeenCalled()
  })

  it('answers failed when the connection is not up in 15 s, and stops the dial', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const signals: AbortSignal[] = []
    const { daemon, answer } = stream(
      (_destination, signal) =>
        new Promise((_resolve, reject) => {
          signals.push(signal)
          signal.addEventListener('abort', () => reject(signal.reason))
        }),
    )
    daemon.write('example.com:443\n')

    await vi.advanceTimersByTimeAsync(14_900)
    expect(answer()).toBe('')
    await vi.advanceTimersByTimeAsync(100)

    await ended(daemon)
    expect(answer()).toMatch(/^failed .*15 s.*\n$/)
    expect(signals[0].aborted).toBe(true)
  })

  it('stops the dial when the stream closes during it', async () => {
    const signals: AbortSignal[] = []
    const { daemon } = stream(
      (_destination, signal) =>
        new Promise((_resolve, reject) => {
          signals.push(signal)
          signal.addEventListener('abort', () => reject(signal.reason))
        }),
    )
    daemon.write('example.com:443\n')
    await vi.waitFor(() => expect(signals).toHaveLength(1))

    daemon.destroy(new Error('the daemon reset the stream'))

    await vi.waitFor(() => expect(signals[0].aborted).toBe(true))
  })
})

/** An exit socket that a test drives. */
class FakeExitSocket implements ByteSocket {
  readonly sent: Buffer[] = []
  closed = false
  private readonly messageListeners: Array<(bytes: Uint8Array) => void> = []
  private readonly closeListeners: Array<(code: number) => void> = []

  send(bytes: Uint8Array): void {
    this.sent.push(Buffer.from(bytes))
  }

  onMessage(listener: (bytes: Uint8Array) => void): void {
    this.messageListeners.push(listener)
  }

  onClose(listener: (code: number) => void): void {
    this.closeListeners.push(listener)
  }

  /** The client closes the socket. */
  close(): void {
    this.closeWith(1005)
  }

  /** The socket closes with a code. */
  closeWith(code: number): void {
    if (this.closed) return
    this.closed = true
    for (const listener of this.closeListeners) listener(code)
  }

  /** The daemon sends bytes. */
  deliver(bytes: Buffer): void {
    for (const listener of this.messageListeners) listener(bytes)
  }

  received(): Buffer {
    return Buffer.concat(this.sent)
  }
}

describe('the exit traffic', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('tells each connection that opens or closes at once, and new bytes at most once a second', () => {
    vi.useFakeTimers()
    const onChange = vi.fn()
    const traffic = new ExitTraffic(onChange)

    traffic.opened()
    expect(onChange).toHaveBeenCalledTimes(1)
    traffic.copied(100)
    traffic.copied(200)
    expect(onChange).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(999)
    expect(onChange).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(1)
    expect(onChange).toHaveBeenCalledTimes(2)
    expect(traffic.bytes).toBe(300)

    traffic.closed()
    expect(onChange).toHaveBeenCalledTimes(3)
    expect(traffic.connections).toBe(0)
    vi.advanceTimersByTime(5_000)
    expect(onChange).toHaveBeenCalledTimes(3)
  })
})

describe('the link that keeps the exit socket open', () => {
  const sites: Array<{ close: () => Promise<void> }> = []
  afterEach(async () => {
    for (const opened of sites.splice(0)) await opened.close()
  })

  function linkOver(
    sockets: FakeExitSocket[],
    dial: Dial = vi.fn(),
    sessionEnded = () => {},
    traffic = new ExitTraffic(),
  ): ExitLink {
    return new ExitLink(
      async () => {
        const socket = new FakeExitSocket()
        sockets.push(socket)
        return socket
      },
      1,
      dial,
      sessionEnded,
      traffic,
    )
  }

  it('opens the socket again after a close', async () => {
    const sockets: FakeExitSocket[] = []
    const link = linkOver(sockets)

    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))
    sockets[0].closeWith(1006)
    await vi.waitFor(() => expect(sockets).toHaveLength(2))

    link.stop()
    expect(sockets[1].closed).toBe(true)
  })

  it('keeps trying while the daemon refuses the socket', async () => {
    let attempts = 0
    const link = new ExitLink(async () => {
      attempts += 1
      if (attempts < 3) throw new Error('the daemon refused the upgrade')
      return new FakeExitSocket()
    }, 1)

    link.start()
    await vi.waitFor(() => expect(attempts).toBe(3))

    link.stop()
  })

  it('sends Go Away, closes the socket and opens nothing more once it is stopped', async () => {
    const sockets: FakeExitSocket[] = []
    const link = linkOver(sockets)
    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    link.stop()
    await new Promise((resolve) => setTimeout(resolve, 20))

    expect(sockets).toHaveLength(1)
    expect(sockets[0].closed).toBe(true)
    expect(yamuxFrames(sockets[0].received())).toEqual([
      { type: GO_AWAY, flags: 0, streamId: 0, length: 0, body: Buffer.alloc(0) },
    ])
  })

  it('reports a close with 1008 as an ended Session, and no other close', async () => {
    const sockets: FakeExitSocket[] = []
    const sessionEnded = vi.fn()
    const link = linkOver(sockets, vi.fn(), sessionEnded)
    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    sockets[0].closeWith(1006)
    await vi.waitFor(() => expect(sockets).toHaveLength(2))
    expect(sessionEnded).not.toHaveBeenCalled()

    sockets[1].closeWith(1008)
    await vi.waitFor(() => expect(sockets).toHaveLength(3))
    expect(sessionEnded).toHaveBeenCalledTimes(1)
    link.stop()
  })

  it('carries each stream that the daemon opens, and closes its connection when the socket ends', async () => {
    const target = await site()
    sites.push(target)
    const sockets: FakeExitSocket[] = []
    const asked: Destination[] = []
    const traffic = new ExitTraffic()
    const link = linkOver(sockets, loopbackDial(target.port, asked), () => {}, traffic)
    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    sockets[0].deliver(Buffer.concat([openStream(1, 'example.com:443\nGET /'), openStream(3, 'example.org:443\n')]))
    await vi.waitFor(() => expect(target.connections).toHaveLength(2))
    await vi.waitFor(() => expect(streamBytes(sockets[0].received(), 1)).toBe('ok\nhello from the site'))
    expect(asked).toEqual([
      { host: 'example.com', port: 443 },
      { host: 'example.org', port: 443 },
    ])
    expect(traffic.connections).toBe(2)

    sockets[0].closeWith(1006)

    await Promise.all(target.connections.map(released))
    await vi.waitFor(() => expect(traffic.connections).toBe(0))
    link.stop()
  })

  it('closes the socket when the daemon breaks the yamux protocol, and opens a new one', async () => {
    const sockets: FakeExitSocket[] = []
    const link = linkOver(sockets)
    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    // A SYN on an even id: the daemon opens odd ids only.
    sockets[0].deliver(openStream(2, 'example.com:443\n'))

    expect(sockets[0].closed).toBe(true)
    await vi.waitFor(() => expect(sockets).toHaveLength(2))
    link.stop()
  })
})

/** The interop harness that a test of the daemon runs: plain node, with
 *  no package, from the root of the repository. */
describe('the interop harness of the daemon', () => {
  const ROOT = path.join(__dirname, '..', '..')
  const resources: Array<{ close: () => Promise<void> }> = []
  afterEach(async () => {
    for (const resource of resources.splice(0)) await resource.close()
  })

  function harness(...args: string[]) {
    const child = spawn(
      process.execPath,
      ['--disable-warning=MODULE_TYPELESS_PACKAGE_JSON', 'desktop/test/exit-peer.ts', ...args],
      { cwd: ROOT, stdio: ['pipe', 'pipe', 'pipe'] },
    )
    const output = { stdout: '', stderr: '' }
    child.stdout.on('data', (chunk: Buffer) => (output.stdout += chunk.toString()))
    child.stderr.on('data', (chunk: Buffer) => (output.stderr += chunk.toString()))
    const exited = new Promise<number | null>((resolve) => child.once('exit', (code) => resolve(code)))
    resources.push({
      close: async () => {
        child.kill()
        await exited
      },
    })
    return { child, output, exited }
  }

  it('serves the exit socket and carries every stream to the target', async () => {
    const target = await site()
    resources.push(target)
    const server = await daemonServer()
    resources.push(server)

    const { child, output, exited } = harness(server.origin, 'secret', 'host-1', `127.0.0.1:${target.port}`)
    await vi.waitFor(() => expect(output.stdout).toBe('ready\n'), { timeout: 10_000 })
    const [end] = server.ends
    expect(end.url).toBe('/api/v1/hosts/host-1/exit')
    expect(end.headers.cookie).toBe('pagis_session=secret')

    end.binary(openStream(1, 'example.com:443\nGET /'))

    await vi.waitFor(() => expect(streamBytes(end.received(), 1)).toBe('ok\nhello from the site'))
    expect(yamuxFrames(end.received())[0]).toMatchObject({ type: WINDOW_UPDATE, streamId: 1, length: 0 })
    await vi.waitFor(() => expect(target.received()).toBe('GET /'))

    child.stdin.end()
    expect(await exited).toBe(0)
    expect(output.stderr).toBe('')
  })

  it('exits when the daemon closes the socket', async () => {
    const server = await daemonServer()
    resources.push(server)
    const { output, exited } = harness(server.origin, 'secret', 'host-1', '127.0.0.1:9')
    await vi.waitFor(() => expect(output.stdout).toBe('ready\n'), { timeout: 10_000 })

    server.ends[0].closeWith(1000)

    expect(await exited).toBe(0)
  })

  it('prints the failure and exits non-zero when the socket does not open', async () => {
    const { output, exited } = harness('http://192.0.2.1:4400/', 'secret', 'host-1', '127.0.0.1:9')

    expect(await exited).not.toBe(0)
    expect(output.stderr).toMatch(/https:\/\//)
    expect(output.stdout).toBe('')
  })
})
