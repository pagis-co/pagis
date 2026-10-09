// The Sessions section: each browser and app that is signed in as the
// person, which one is this one, and a way to sign one more in.
//
// A client signs in with a Sign-In Link that a signed-in client makes
// (ADR-0028): the person scans the QR code with a phone, or opens the
// link in another browser. The link is good for five minutes and one
// use, and the client that opens it gets a Session of its own, which
// this list shows and removes.

import { useState } from 'react'

import type { ApiClient, MySessionDto } from '../../api/client'
import { Badge, Button, Dialog, Frame, Row } from '../../primitives'
import {
  errorMessage,
  useEndMySession,
  useMakeSignInLink,
  useMySessions,
} from '../../queries'
import { SettingsSection } from './SettingsSection'
import { SignInLinkCard } from './SignInLinkCard'

import './Sessions.css'
import { useIsMobile } from '../../state/useIsMobile'
import { sinceWhen } from '../when'

/** A time the person reads, in their own browser's zone. */
function when(at: number): string {
  return new Date(at).toLocaleString()
}

/** What the list calls a Session: the browser and its system, or the
 *  Client App and its machine. */
export function sessionLabel(
  session: Pick<MySessionDto, 'client_kind' | 'client_name'>,
): string {
  if (session.client_kind === 'desktop') {
    return session.client_name ? `Client App on ${session.client_name}` : 'Client App'
  }
  return session.client_name ?? 'Browser'
}

function SessionRow({ api, session }: { api: ApiClient; session: MySessionDto }) {
  const phone = useIsMobile()
  const end = useEndMySession(api)
  const label = sessionLabel(session)
  if (phone)
    return (
      <Row data-testid="session-row">
        <span className="phone-row-copy">
          <span>
            {label} {session.current && <Badge tone="accent">This session</Badge>}
          </span>
          <span className="phone-hint">
            signed in {sinceWhen(session.created_at)} · last used {sinceWhen(session.last_used_at)}
          </span>
          {end.isError && (
            <span role="alert">
              {errorMessage(end.error, 'That session could not be removed.')}
            </span>
          )}
        </span>
        {!session.current && (
          <Button
            variant="link"
            className="phone-danger"
            disabled={end.isPending}
            aria-label={`Remove ${label}`}
            onClick={() => end.mutate(session.id)}
          >
            Remove
          </Button>
        )}
      </Row>
    )

  return (
    <Row className="my-session-row" data-testid="session-row">
      <span className="my-session-name">{label}</span>
      {session.current && <Badge tone="working">This session</Badge>}
      <span className="my-session-time">signed in {when(session.created_at)}</span>
      <span className="my-session-time">last used {when(session.last_used_at)}</span>
      {!session.current && (
        <Button
          size="sm"
          variant="danger-quiet"
          className="my-session-remove"
          aria-label={`Remove ${label}`}
          disabled={end.isPending}
          onClick={() => end.mutate(session.id)}
        >
          Remove
        </Button>
      )}
      {end.isError && (
        <span className="settings-error" role="alert">
          {errorMessage(end.error, 'That session could not be removed.')}
        </span>
      )}
    </Row>
  )
}

export function Sessions({ api }: { api: ApiClient }) {
  const phone = useIsMobile()
  const sessions = useMySessions(api)
  const make = useMakeSignInLink(api)
  const [showing, setShowing] = useState(false)
  const items = sessions.data ?? []

  return (
    <SettingsSection
      title="Sessions"
      lead="The browsers and apps that are signed in as you."
      action={
        !phone && <Button
          size="sm"
          variant="primary"
          disabled={make.isPending}
          onClick={() => make.mutate(undefined, { onSuccess: () => setShowing(true) })}
        >
          Sign in another browser or app
        </Button>
      }
      hint="A session ends 30 days after its last use. Remove a session to sign that browser or app out at once."
    >
      <Frame>
        {items.length === 0 ? (
          <Row>
            <span className="settings-hint">
              {sessions.isError ? 'Your sessions could not be read.' : 'Reading…'}
            </span>
          </Row>
        ) : (
          items.map((session) => <SessionRow key={session.id} api={api} session={session} />)
        )}
      </Frame>
      {make.isError && (
        <span className="settings-error" role="alert">
          {errorMessage(make.error, 'A sign-in link could not be made.')}
        </span>
      )}
      {make.data !== undefined && (
        <Dialog
          open={showing}
          onOpenChange={(open) => {
            setShowing(open)
            // The client that opened the link has a Session now.
            if (!open) void sessions.refetch()
          }}
          title="Sign in another browser or app"
          description="Scan the code with the camera of a phone, or open the link in the other browser. It signs in as you."
        >
          <SignInLinkCard link={make.data} />
        </Dialog>
      )}
    </SettingsSection>
  )
}
