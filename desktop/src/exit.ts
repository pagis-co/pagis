// This client as the Home Exit of its Person.
//
// A Computer on a server reaches the internet through the Home Exit, so
// that sites see the Person's own connection and not a data-center
// address. The daemon opens one yamux stream for each connection of the
// Computer over the exit socket, a second WebSocket of this client. For
// each stream the client reads the destination, resolves the name,
// refuses an address on this machine or on its local network, dials, and
// copies the bytes both ways. The HTTP stays in the Exit Proxy inside the
// Computer: this side carries bytes and nothing else.
//
// The stream protocol, which the daemon implements the same way:
//
// 1. The daemon writes the preamble: the destination as `host:port` and
//    one line feed, at most 262 bytes with the line feed. The host is a
//    DNS name, an IPv4 address, or an IPv6 address in brackets.
// 2. The client answers with one status line before any other byte:
//    `ok`, `refused <reason>` or `failed <reason>`, at most 512 bytes with
//    its line feed. `refused` means that every address of the name is on
//    this machine or on a local network. `failed` means a malformed
//    preamble, a name that does not resolve, or a connection that failed
//    or timed out.
// 3. After `ok`, the stream carries the bytes of the TCP connection both
//    ways. After `refused` or `failed`, the client ends the stream.
//
// This module uses only erasable TypeScript syntax and imports only Node
// built-ins, `./byteSocket`, `./streamLine` and `./yamux`, so plain `node`
// loads it in the interop test of the daemon (`test/exit-peer.ts`).

import { lookup } from 'node:dns/promises'
import { BlockList, connect, isIPv4, isIPv6 } from 'node:net'
import type { Socket } from 'node:net'
import { networkInterfaces } from 'node:os'
import type { NetworkInterfaceInfo } from 'node:os'
import { pipeline } from 'node:stream'
import type { Duplex } from 'node:stream'

import type { ByteSocket } from './byteSocket'
import { openByteSocket } from './byteSocket'
import { readLine } from './streamLine'
import { YamuxSession } from './yamux'

/** Where a stream goes. The host of an IPv6 address has no brackets. */
export interface Destination {
  host: string
  port: number
}

/**
 * Open the TCP connection of one stream. It rejects with a
 * [`RefusedDestination`] when the address check refuses the destination,
 * and with the reason of the signal when the signal aborts, which it does
 * when the connection is not up in time or when the stream closes first.
 */
export type Dial = (destination: Destination, signal: AbortSignal) => Promise<Socket>

/** The addresses of a name, as `dns.lookup` with `all` answers them. */
export type Lookup = (host: string) => Promise<readonly { address: string }[]>

/** Open a TCP connection to an address, as [`connectTcp`] does. */
export type Connect = (host: string, port: number, signal: AbortSignal) => Promise<Socket>

/** The interfaces of this machine, as `os.networkInterfaces` gives them. */
export type Interfaces = () => Partial<Record<string, readonly NetworkInterfaceInfo[]>>

/** A dial that the address check refused: the answer is `refused`. */
export class RefusedDestination extends Error {}

/** The longest preamble, with its line feed: `host:port\n`. */
const MAX_PREAMBLE = 262

/** The longest status line, with its line feed. */
const MAX_STATUS = 512

const PREAMBLE_TIMEOUT_MS = 10_000
const DIAL_TIMEOUT_MS = 15_000

/** How long the traffic waits before it tells of new bytes, so a page
 *  that loads tells of its bytes once a second at most. */
const TRAFFIC_NOTICE_MS = 1_000

/** The close code of an exit socket whose Session ended, the same code as
 *  that of the Host socket: 1008, policy violation. */
const SESSION_ENDED = 1008

/** The networks on this machine or on a local network. The IPv4 ones:
 *  this network, private, carrier-grade NAT, loopback, link-local,
 *  multicast and broadcast. An IPv4-mapped IPv6 address matches them too. */
const REFUSED = new BlockList()
REFUSED.addSubnet('0.0.0.0', 8, 'ipv4')
REFUSED.addSubnet('10.0.0.0', 8, 'ipv4')
REFUSED.addSubnet('100.64.0.0', 10, 'ipv4')
REFUSED.addSubnet('127.0.0.0', 8, 'ipv4')
REFUSED.addSubnet('169.254.0.0', 16, 'ipv4')
REFUSED.addSubnet('172.16.0.0', 12, 'ipv4')
REFUSED.addSubnet('192.168.0.0', 16, 'ipv4')
REFUSED.addSubnet('224.0.0.0', 4, 'ipv4')
REFUSED.addAddress('255.255.255.255', 'ipv4')
// The IPv6 ones: unspecified, loopback, unique local, link-local and
// multicast.
REFUSED.addAddress('::', 'ipv6')
REFUSED.addAddress('::1', 'ipv6')
REFUSED.addSubnet('fc00::', 7, 'ipv6')
REFUSED.addSubnet('fe80::', 10, 'ipv6')
REFUSED.addSubnet('ff00::', 8, 'ipv6')

/** The NAT64 prefix. A NAT64 gateway at home sends an address in it to
 *  the IPv4 address of its last 32 bits. */
const NAT64 = new BlockList()
NAT64.addSubnet('64:ff9b::', 96, 'ipv6')

/**
 * Whether the client refuses to connect to an address: one on this
 * machine or on a local network, in either family, an IPv4-mapped or
 * NAT64 form of one, or a string that is not an address at all.
 */
export function isRefusedAddress(address: string): boolean {
  if (isIPv4(address)) return REFUSED.check(address, 'ipv4')
  // Only a link-local or a multicast address has a zone.
  if (!isIPv6(address) || address.includes('%')) return true
  if (REFUSED.check(address, 'ipv6')) return true
  return NAT64.check(address, 'ipv6') && REFUSED.check(lastIPv4(address), 'ipv4')
}

/** The IPv4 address in the last 32 bits of an IPv6 address. */
function lastIPv4(address: string): string {
  // The URL parser writes the address in its shortest hex form, with no
  // dotted IPv4 part.
  const hex = new URL(`http://[${address}]/`).hostname.slice(1, -1)
  const [head, tail] = hex.includes('::') ? hex.split('::') : [hex, '']
  const groups = [head, tail].map((part) => (part === '' ? [] : part.split(':')))
  const words = [...groups[0], ...Array<string>(8 - groups[0].length - groups[1].length).fill('0'), ...groups[1]]
  const high = parseInt(words[6], 16)
  const low = parseInt(words[7], 16)
  return [high >> 8, high & 0xff, low >> 8, low & 0xff].join('.')
}

/**
 * The networks of this machine: each address of each interface, and the
 * subnet of each interface, in both families.
 *
 * A home network can have public addresses: most have global IPv6
 * addresses, and some machines have a public IPv4 address. The fixed
 * ranges of [`isRefusedAddress`] do not hold them, and these networks do.
 */
function localNetworks(interfaces: Interfaces): BlockList {
  const local = new BlockList()
  for (const addresses of Object.values(interfaces())) {
    for (const entry of addresses ?? []) {
      const family = String(entry.family) === 'IPv4' || String(entry.family) === '4' ? 'ipv4' : 'ipv6'
      const address = entry.address.split('%')[0]
      local.addAddress(address, family)
      const prefix = entry.cidr?.split('/')[1]
      if (prefix !== undefined) local.addSubnet(address, Number(prefix), family)
    }
  }
  return local
}

/** A DNS name, as the Exit Proxy writes it in a preamble. */
const NAME = /^[A-Za-z0-9._-]+$/

/**
 * The destination in a preamble line, without its line feed: `host:port`
 * as in the target of a `CONNECT` request.
 */
export function parseDestination(line: string): Destination {
  const colon = line.lastIndexOf(':')
  const hostPart = colon === -1 ? '' : line.slice(0, colon)
  const portPart = colon === -1 ? '' : line.slice(colon + 1)
  const port = /^[0-9]{1,5}$/.test(portPart) ? Number(portPart) : 0
  if (port < 1 || port > 65_535) throw new Error(`the destination ${JSON.stringify(line)} has no port`)
  if (hostPart.startsWith('[') && hostPart.endsWith(']')) {
    const host = hostPart.slice(1, -1)
    if (isIPv6(host)) return { host, port }
  } else if (NAME.test(hostPart)) {
    return { host: hostPart, port }
  }
  throw new Error(`the destination ${JSON.stringify(line)} is not a host and a port`)
}

/**
 * Open a TCP connection to `host` and `port`.
 *
 * Each side of the connection can close its half alone, as a stream can.
 * Small writes go out at once, as the client in the Computer wrote them.
 */
export function connectTcp(host: string, port: number, signal: AbortSignal): Promise<Socket> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(signal.reason)
      return
    }
    const socket = connect({ host, port, allowHalfOpen: true, noDelay: true })
    const fail = (error: Error): void => {
      signal.removeEventListener('abort', abort)
      socket.destroy()
      reject(error)
    }
    const abort = (): void => fail(signal.reason as Error)
    socket.once('error', fail)
    signal.addEventListener('abort', abort, { once: true })
    socket.once('connect', () => {
      socket.off('error', fail)
      signal.removeEventListener('abort', abort)
      resolve(socket)
    })
  })
}

/**
 * The dial of the Home Exit.
 *
 * It resolves the name here, so the name resolves where the connection
 * leaves, and it checks every address after the lookup, so a name that
 * resolves to the home network is refused. The check refuses the fixed
 * ranges of [`isRefusedAddress`], each address of this machine, and each
 * address in the subnet of one of its interfaces. It reads the interfaces
 * at each dial, because they change as a laptop moves. It connects to
 * the first address that passes, by address, so no second lookup can
 * give another one. A literal address resolves to itself.
 */
export function homeDial(
  lookupName: Lookup = (host) => lookup(host, { all: true }),
  connectTo: Connect = connectTcp,
  interfaces: Interfaces = networkInterfaces,
): Dial {
  return async ({ host, port }, signal) => {
    const addresses = await untilAborted(lookupName(host), signal)
    const local = localNetworks(interfaces)
    const allowed = addresses.find(
      ({ address }) =>
        !isRefusedAddress(address) && !local.check(address, isIPv4(address) ? 'ipv4' : 'ipv6'),
    )
    if (allowed !== undefined) return connectTo(allowed.address, port, signal)
    if (addresses.length === 0) throw new Error(`${host} has no address`)
    throw new RefusedDestination(`every address of ${host} is on this machine or on a local network`)
  }
}

function untilAborted<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(signal.reason)
      return
    }
    const abort = (): void => reject(signal.reason)
    signal.addEventListener('abort', abort, { once: true })
    work.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort))
  })
}

/**
 * The exit traffic that this client carries for its Person's Computers:
 * the connections open now, and the bytes that it copied both ways since
 * the client started. The tray item says when traffic flows, with the
 * byte count.
 *
 * `onChange` hears each connection that opens or closes at once, and new
 * bytes once a second at most.
 */
export class ExitTraffic {
  private readonly onChange: () => void
  private open = 0
  private total = 0
  private timer: ReturnType<typeof setTimeout> | null = null

  constructor(onChange: () => void = () => {}) {
    this.onChange = onChange
  }

  /** The connections that this client carries now. */
  get connections(): number {
    return this.open
  }

  /** The bytes that this client copied both ways since it started. */
  get bytes(): number {
    return this.total
  }

  opened(): void {
    this.open += 1
    this.onChange()
  }

  closed(): void {
    this.open -= 1
    this.onChange()
  }

  copied(count: number): void {
    this.total += count
    if (this.timer !== null) return
    this.timer = setTimeout(() => {
      this.timer = null
      this.onChange()
    }, TRAFFIC_NOTICE_MS)
  }
}

/**
 * Carry the connection of one stream: read its destination, dial it,
 * answer, and copy the bytes both ways. `traffic` counts the connection
 * while it is open, and the bytes that it copies.
 *
 * A FIN on the stream half-closes the socket, and the end of the socket's
 * readable side ends the stream. An error on either side destroys both.
 */
export async function carry(stream: Duplex, dial: Dial, traffic?: ExitTraffic): Promise<void> {
  // A reset of the stream or of its session is how a connection ends
  // early. The 'close' that follows it stops whatever the stream feeds.
  stream.on('error', () => {})
  let destination: Destination
  try {
    destination = parseDestination(await readLine(stream, MAX_PREAMBLE, PREAMBLE_TIMEOUT_MS))
  } catch (error) {
    answer(stream, 'failed', error)
    return
  }
  let socket: Socket
  try {
    socket = await dialInTime(stream, dial, destination)
  } catch (error) {
    answer(stream, error instanceof RefusedDestination ? 'refused' : 'failed', error)
    return
  }
  if (stream.destroyed) {
    socket.destroy()
    return
  }
  stream.write('ok\n')
  // Each pipeline destroys both sides on an error of either. A reset on
  // either side is the end of the connection and not a fault of the
  // client, so the callbacks have nothing more to do.
  pipeline(stream, socket, () => {})
  pipeline(socket, stream, () => {})
  if (traffic !== undefined) {
    traffic.opened()
    stream.once('close', () => traffic.closed())
    // The pipelines read in the same turn, so these listeners see every
    // chunk that they copy.
    stream.on('data', (chunk: Buffer) => traffic.copied(chunk.length))
    socket.on('data', (chunk: Buffer) => traffic.copied(chunk.length))
  }
}

/** Dial, and stop the dial when it is not up in time or when the stream
 *  closes first. */
async function dialInTime(stream: Duplex, dial: Dial, destination: Destination): Promise<Socket> {
  const controller = new AbortController()
  const timer = setTimeout(
    () => controller.abort(new Error(`no connection to ${destination.host} in ${DIAL_TIMEOUT_MS / 1000} s`)),
    DIAL_TIMEOUT_MS,
  )
  const closed = (): void => controller.abort(new Error('the stream closed'))
  stream.once('close', closed)
  try {
    return await dial(destination, controller.signal)
  } finally {
    clearTimeout(timer)
    stream.off('close', closed)
  }
}

/** Answer `refused` or `failed` and end the stream. */
function answer(stream: Duplex, status: 'refused' | 'failed', error: unknown): void {
  if (stream.destroyed || stream.writableEnded) return
  const reason = (error instanceof Error ? error.message : String(error)).replace(/[^\x20-\x7e]/g, ' ')
  stream.end(`${`${status} ${reason}`.slice(0, MAX_STATUS - 1)}\n`)
  // Nothing reads the rest of the stream, so it can close once the
  // daemon closes its side.
  stream.resume()
}

/**
 * The exit socket of the Host with the id `hostId`, a [`ByteSocket`] on
 * the exit path.
 */
export function openExitSocket(url: string, sessionSecret: string, hostId: string): Promise<ByteSocket> {
  return openByteSocket(url, sessionSecret, `/api/v1/hosts/${encodeURIComponent(hostId)}/exit`)
}

/**
 * Keep the exit socket open for as long as the client runs, and carry
 * each stream that the daemon opens on it. `traffic` counts what the
 * streams carry.
 *
 * A socket that closes takes its streams and their connections with it,
 * and the link opens another one. A socket that closes with code 1008
 * lost its Session: the link tells `sessionEnded`, and the next `open`
 * must not use that Session.
 */
export class ExitLink {
  private readonly open: () => Promise<ByteSocket>
  private readonly retryMs: number
  private readonly dial: Dial
  private readonly sessionEnded: () => void
  private readonly traffic: ExitTraffic
  private socket: ByteSocket | null = null
  private session: YamuxSession | null = null
  private stopped = false
  private timer: ReturnType<typeof setTimeout> | null = null

  constructor(
    open: () => Promise<ByteSocket>,
    retryMs = 3_000,
    dial: Dial = homeDial(),
    sessionEnded: () => void = () => {},
    traffic: ExitTraffic = new ExitTraffic(),
  ) {
    this.open = open
    this.retryMs = retryMs
    this.dial = dial
    this.sessionEnded = sessionEnded
    this.traffic = traffic
  }

  /** Open the socket, and keep doing so until [`stop`]. */
  start(): void {
    this.stopped = false
    void this.connect()
  }

  /** Close the socket and every connection that it carries. */
  stop(): void {
    this.stopped = true
    if (this.timer !== null) clearTimeout(this.timer)
    this.timer = null
    this.session?.close()
    this.session = null
    this.socket?.close()
    this.socket = null
  }

  private async connect(): Promise<void> {
    if (this.stopped) return
    let socket: ByteSocket
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
    const session = new YamuxSession({
      send: (bytes) => socket.send(bytes),
      onStream: (stream) => void carry(stream, this.dial, this.traffic),
      // The daemon ended the session or broke the protocol. A new socket
      // starts a new session.
      onEnd: () => socket.close(),
    })
    this.socket = socket
    this.session = session
    socket.onMessage((bytes) => session.receive(bytes))
    socket.onClose((code) => {
      session.abort()
      if (this.socket !== socket) return
      this.socket = null
      this.session = null
      if (code === SESSION_ENDED) this.sessionEnded()
      this.retry()
    })
  }

  private retry(): void {
    if (this.stopped || this.timer !== null) return
    this.timer = setTimeout(() => {
      this.timer = null
      void this.connect()
    }, this.retryMs)
  }
}
