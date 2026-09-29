// Who is signed in, and which machines can act: the
// person, the kind of client they signed in from, and since when.
//
// A Session says who asks and grants nothing on its own, so this view
// answers one question: which clients hold one right now. A Client App
// Session names the machine it runs on. The Session of this page is
// marked, so the list never reads as somebody else's client.
//
// The Hosts beneath it answer the other half. A host action runs on a
// person's own connected client and never on this server, so an
// administrator who wants to know whether such an action can run at all
// reads presence here.

import type { AdministrationHostDto, ApiClient, SessionDto } from '../api/client'
import { Badge, Frame, Row, SectionLabel } from '../primitives'
import { useInstallationHosts, useLiveSessions } from '../queries'
import { personLabel } from '../components/settings/People'
import { platformLabel } from '../components/settings/Hosts'

const CLIENT_NAMES: Record<string, string> = {
  browser: 'Browser',
  desktop: 'Client App',
}

/** A time a person reads, in their own browser's zone. */
function when(at: number): string {
  return new Date(at).toLocaleString()
}

/** The kind of client, and the machine of a Client App. */
function clientLabel(session: SessionDto): string {
  const kind = CLIENT_NAMES[session.client_kind] ?? session.client_kind
  return session.client_name ? `${kind} · ${session.client_name}` : kind
}

function SessionRow({ session }: { session: SessionDto }) {
  return (
    <Row className="sessions-row">
      <span className="sessions-person">{personLabel(session.person)}</span>
      <Badge tone={session.client_kind === 'desktop' ? 'accent' : 'neutral'}>
        {clientLabel(session)}
      </Badge>
      <span className="sessions-since">since {when(session.created_at)}</span>
      <span className="sessions-used">last used {when(session.last_used_at)}</span>
      {session.current && <Badge tone="working">This session</Badge>}
    </Row>
  )
}

function HostRow({ host }: { host: AdministrationHostDto }) {
  return (
    <Row className="sessions-row">
      <span className="sessions-person">{personLabel(host.person)}</span>
      <span className="sessions-since">{host.name}</span>
      <span className="sessions-used">{platformLabel(host.platform)}</span>
      <Badge tone={host.present ? 'working' : 'neutral'}>
        {host.present ? 'Connected' : `last seen ${when(host.last_seen_at)}`}
      </Badge>
    </Row>
  )
}

/** Every machine of the installation, with whether a host action could
 *  run on it now. */
export function Hosts({ api }: { api: ApiClient }) {
  const hosts = useInstallationHosts(api)
  const items = hosts.data ?? []
  const present = items.filter((host) => host.present).length

  return (
    <section className="administration-section">
      <div className="administration-title">
        <h2>Hosts</h2>
        <span>The computers of the people here, and which are connected.</span>
      </div>
      <Frame hint="A host action runs on a person's own computer, through the Pagis client running on it, and never on this server. A computer that is not connected runs nothing until its client is open again.">
        {items.length === 0 ? (
          <Row>
            <span className="administration-note">
              {hosts.isError
                ? 'The hosts could not be read.'
                : 'No computer is registered.'}
            </span>
          </Row>
        ) : (
          items.map((host) => <HostRow key={host.id} host={host} />)
        )}
      </Frame>
      <SectionLabel>
        {present} of {items.length} connected
      </SectionLabel>
    </section>
  )
}

export function Sessions({ api }: { api: ApiClient }) {
  const sessions = useLiveSessions(api)
  const items = sessions.data ?? []

  return (
    <section className="administration-section">
      <div className="administration-title">
        <h2>Sessions</h2>
        <span>Who is signed in, and from what.</span>
      </div>
      <Frame hint="A Session ends when the person signs out and expires by itself after thirty days. Disabling an account or resetting its password ends every Session of that person at once.">
        {items.length === 0 ? (
          <Row>
            <span className="administration-note">
              {sessions.isError
                ? 'The sessions could not be read.'
                : 'Nobody is signed in.'}
            </span>
          </Row>
        ) : (
          items.map((session) => <SessionRow key={session.id} session={session} />)
        )}
      </Frame>
      <SectionLabel>{items.length} signed in</SectionLabel>
      <Hosts api={api} />
    </section>
  )
}
