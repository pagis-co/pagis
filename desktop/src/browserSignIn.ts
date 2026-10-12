import { createHash, randomBytes } from 'node:crypto'
import type { ServerConnection } from './serverConnection'

interface PendingSignIn {
  server: ServerConnection
  verifier: string
  made: number
}

export interface AcceptedSignIn {
  server: ServerConnection
  url: string
  secret: string
  verifier: string
}

/** State and verifier live only in memory. Each state works once for five minutes. */
export class BrowserSignIns {
  private readonly pending = new Map<string, PendingSignIn>()
  constructor(private readonly now: () => number = Date.now) {}

  begin(server: ServerConnection): string {
    this.expire()
    const verifier = randomBytes(32).toString('base64url')
    const state = randomBytes(32).toString('base64url')
    this.pending.set(state, { server, verifier, made: this.now() })
    const url = new URL('/sign-in', server.origin)
    url.search = new URLSearchParams({ app: 'desktop', state, challenge: createHash('sha256').update(verifier).digest('base64url') }).toString()
    return url.toString()
  }

  accept(link: string): AcceptedSignIn | null {
    this.expire()
    let url: URL
    let secret: string
    try { url = new URL(link); secret = decodeURIComponent(url.hash.slice(1)) } catch { return null }
    if (url.protocol !== 'pagis:' || url.host !== 'auth' || url.username || url.password || url.pathname ||
      [...url.searchParams.keys()].join(',') !== 'state' || !secret) return null
    const state = url.searchParams.get('state')!
    const pending = this.pending.get(state)
    if (!pending) return null
    this.pending.delete(state)
    const opens = new URL('/sign-in', pending.server.origin)
    opens.hash = encodeURIComponent(secret)
    return { server: pending.server, verifier: pending.verifier, secret, url: opens.toString() }
  }

  remove(id: string): void {
    for (const [state, pending] of this.pending) if (pending.server.id === id) this.pending.delete(state)
  }

  private expire(): void {
    for (const [state, pending] of this.pending) if (this.now() - pending.made >= 300_000) this.pending.delete(state)
  }
}
