// The Hosts section: the person's own machines, which of them a sprite
// can act on right now, and the Coding Harnesses that each one can start.
//
// A host action runs on a client the person is running and never on the
// server, so "connected" is the whole of what this page answers. A
// machine that is not connected still holds its grants and its allow
// rules; it simply cannot be asked to do anything until the client on it
// is open again.

import { Fragment } from 'react'

import { Monitor } from 'lucide-react'
import type { ApiClient, HostDto } from '../../api/client'
import { Badge, Frame, Row, SectionLabel } from '../../primitives'
import { useHosts } from '../../queries'
import { useIsMobile } from '../../state/useIsMobile'
import { sinceWhen } from '../when'
import { HostHarnesses } from './HostHarnesses'

/** What the person calls the system the client reported. */
const PLATFORM_NAMES: Record<string, string> = {
  macos: 'macOS',
  linux: 'Linux',
  windows: 'Windows',
  ios: 'iOS',
  android: 'Android',
}

export function platformLabel(platform: string): string {
  return PLATFORM_NAMES[platform] ?? platform
}

export const HOST_HINT = "A sprite runs a command on a computer of yours through the Pagis client running on it, never on the server. Open the client on a computer to make it available, and quit the client to take it away. A sign-in runs in the harness's own program on your computer. Pagis never sees your password or your key."

/** A time the person reads, in their own browser's zone. */
function when(at: number): string {
  return new Date(at).toLocaleString()
}

export function HostRow({ host }: { host: HostDto }) {
  const phone = useIsMobile()
  const canShell = host.capabilities.includes('shell')
  if (phone)
    return (
      <Row data-testid="host-row">
        <span className="phone-initial-tile">
          <Monitor size={20} aria-hidden />
        </span>
        <span className="phone-row-copy">
          <strong>{host.name}</strong>
          <span className="phone-hint">
            {platformLabel(host.platform)} · {canShell ? 'Runs commands' : 'No commands'}
            {!host.present ? ` · last seen ${sinceWhen(host.last_seen_at)}` : ''}
          </span>
        </span>
        <Badge tone={host.present ? 'working' : 'neutral'}>
          {host.present ? 'Connected' : 'Not connected'}
        </Badge>
      </Row>
    )
  return (
    <Row className="host-row" data-testid="host-row">
      <span className="host-name">{host.name}</span>
      <span className="host-platform">{platformLabel(host.platform)}</span>
      <Badge tone={host.present ? 'working' : 'neutral'}>
        {host.present ? 'Connected' : 'Not connected'}
      </Badge>
      {canShell ? (
        <span className="host-capability">Runs commands</span>
      ) : (
        <span className="host-capability">No commands</span>
      )}
      {!host.present && (
        <span className="host-seen">last seen {when(host.last_seen_at)}</span>
      )}
    </Row>
  )
}

export function Hosts({ api }: { api: ApiClient }) {
  const phone = useIsMobile()
  const hosts = useHosts(api)
  const items = hosts.data ?? []
  const present = items.filter((host) => host.present).length

  return (
    <section className="settings-section hosts">
      <div className="hosts-title">
        <h1>Hosts</h1>
        <span>The computers your sprites can act on.</span>
      </div>
      <Frame hint={phone ? undefined : HOST_HINT}>
        {items.length === 0 ? (
          <Row>
            <span className="settings-hint">
              {hosts.isError
                ? 'Your computers could not be read.'
                : 'No computer is registered. Open the Pagis client on a computer to add it.'}
            </span>
          </Row>
        ) : (
          items.map((host) => (
            <Fragment key={host.id}>
              <HostRow host={host} />
              <HostHarnesses api={api} host={host} />
            </Fragment>
          ))
        )}
      </Frame>
      {phone ? <p className="phone-hint">{present} of {items.length} connected</p> : <SectionLabel>{present} of {items.length} connected</SectionLabel>}
    </section>
  )
}
