// Remote Access (ADR-0028): one switch that serves this installation to
// the owner's other machines and the other People, at the public name
// of the owner's Tailscale Funnel.
//
// The switch reads the Tailscale of this computer and says what to do in
// each state. Turning it on runs Funnel in the background, for Pagis on
// port 443 and for the TURN server of the live screen on port 8443,
// because Tailscale can wait while the owner turns on HTTPS and Funnel
// for the tailnet on a page that it names; the switch shows that page
// until the turn-on ends. Either change takes effect on a restart, which
// the switch asks for. A server shows Remote Access and no switch: its
// deployment sets PAGIS_REMOTE_ACCESS.

import { useEffect, useState } from 'react'

import type { ApiClient, RemoteAccessDto, ScreenDto, TailscaleState } from '../../api/client'
import { Badge, Button, Frame, Row, Switch } from '../../primitives'
import {
  errorMessage,
  useRemoteAccess,
  useTurnOffRemoteAccess,
  useTurnOnRemoteAccess,
} from '../../queries'
import { type RunningDaemon, restartMessage, useDaemonRestart } from './restart'

/** The anchor of the switch in the Settings view. The Client App opens
 *  `/settings#remote-access` after a setup for several People. */
export const REMOTE_ACCESS_ANCHOR = 'remote-access'

/** The documentation page of Remote Access. */
export const REMOTE_ACCESS_GUIDE = 'https://docs.pagis.co/client-app/several-people'

/** The documentation page of the Media Relay of a server. */
export const SCREEN_GUIDE = 'https://docs.pagis.co/server/live-screen'

/** The two ports of the Funnel: what each publishes of Pagis, and what
 *  removes it by hand. */
const FUNNEL_PORTS = [
  { key: 'port_443', number: 443, off: 'tailscale funnel --https=443 off' },
  { key: 'port_8443', number: 8443, off: 'tailscale funnel --tls-terminated-tcp=8443 off' },
] as const

/** What each port of the Funnel serves, where Tailscale runs. */
function funnelPorts(tailscale: TailscaleState | null | undefined) {
  if (tailscale?.state !== 'ready' && tailscale?.state !== 'funnel_off') return []
  return FUNNEL_PORTS.map((port) => ({ ...port, served: tailscale[port.key] }))
}

/** Whether the switch can turn Remote Access on in this state. */
function canTurnOn(tailscale: TailscaleState | null | undefined): boolean {
  if (tailscale?.state !== 'ready' && tailscale?.state !== 'funnel_off') return false
  return funnelPorts(tailscale).every((port) => port.served.serves !== 'other')
}

/** What the Tailscale of this computer asks the owner to do, with the
 *  one action of each state. */
function TailscaleNote({
  tailscale,
  onCheckAgain,
  checking,
}: {
  tailscale: TailscaleState
  onCheckAgain: () => void
  checking: boolean
}) {
  const checkAgain = (
    <Button variant="ghost" disabled={checking} onClick={onCheckAgain}>
      Check again
    </Button>
  )
  const taken = funnelPorts(tailscale).find((port) => port.served.serves === 'other')
  if (taken?.served.serves === 'other') {
    return (
      <Row>
        <span className="system-row-note">
          Port {taken.number} of Tailscale Funnel on this computer serves{' '}
          <code>{taken.served.target}</code>. Pagis does not replace it. Remove it with{' '}
          <code>{taken.off}</code>, then check again.
        </span>
        {checkAgain}
      </Row>
    )
  }
  switch (tailscale.state) {
    case 'not_installed':
      return (
        <Row>
          <span className="system-row-note">
            Tailscale is not on this computer. Remote Access runs through your own Tailscale
            account. Install{' '}
            <a href={tailscale.install_url} target="_blank" rel="noreferrer">
              Tailscale
            </a>
            , sign in, then check again.
          </span>
          {checkAgain}
        </Row>
      )
    case 'not_running':
      return (
        <Row>
          <span className="system-row-note">
            Tailscale does not run on this computer, or it is signed out. Open Tailscale and
            sign in, then check again.
            {tailscale.detail && (
              <>
                {' '}
                Tailscale says: <span className="system-mono">{tailscale.detail}</span>
              </>
            )}
          </span>
          {checkAgain}
        </Row>
      )
    case 'funnel_off':
      return (
        <Row>
          <span className="system-row-note">
            HTTPS and Funnel are off for your tailnet. When you turn on Remote Access, Tailscale
            names a page where you turn them on.
          </span>
        </Row>
      )
    case 'ready':
      return (
        <Row>
          <span className="system-row-note">
            Tailscale is ready. Other machines will reach Pagis at{' '}
            <span className="system-mono">https://{tailscale.dns_name}</span>.
          </span>
        </Row>
      )
  }
}

/** What another machine sees of the live screen of a Computer. The
 *  Funnel carries TCP alone, so the TURN server of Pagis on port 8443
 *  carries the screen to it, unless the machine reaches the Media Relay
 *  over UDP. */
function ScreenNote({ screen }: { screen: ScreenDto }) {
  return (
    <Row>
      <span className="system-row-note">
        Other machines see the live screen of a Computer through the TURN server of Pagis, on
        port 8443 of the Funnel. The Funnel carries about 15 Mbit/s in total for this computer,
        and each viewer takes about 2 Mbit/s, so a few people watch at once.{' '}
        {screen.relay === 'turn' ? (
          <>Machines that reach your TURN server use it first.</>
        ) : (
          !screen.loopback && (
            <>
              Machines that reach <code>{screen.advertise_ip}</code> over UDP ports{' '}
              <code>
                {screen.media_port_first}–{screen.media_port_last}
              </code>{' '}
              use that direct path first.
            </>
          )
        )}
      </span>
    </Row>
  )
}

/** A server: Remote Access as its deployment sets it, and no switch. */
function ServerRemoteAccess({ remote, screen }: { remote: RemoteAccessDto; screen: ScreenDto }) {
  return (
    <Frame id={REMOTE_ACCESS_ANCHOR}>
      <Row>
        <Badge tone={remote.enabled ? 'accent' : 'neutral'}>
          Remote Access {remote.enabled ? 'on' : 'off'}
        </Badge>
        <span className="system-row-note">
          This Pagis is a server. Its deployment names the Public Origin, and{' '}
          <code>PAGIS_REMOTE_ACCESS</code> turns on Remote Access.
        </span>
      </Row>
      {remote.public_origin && (
        <Row>
          <span className="system-about-key">Public name</span>
          <span className="system-mono">{remote.public_origin}</span>
        </Row>
      )}
      <Row>
        <span className="system-row-note">
          {screen.relay === 'turn'
            ? 'The live screen of a Computer goes through your TURN server.'
            : `The Media Relay advertises ${screen.advertise_ip} on UDP ports ${screen.media_port_first}–${screen.media_port_last}.`}{' '}
          <a href={SCREEN_GUIDE} target="_blank" rel="noreferrer">
            Live screen
          </a>
          .
        </span>
      </Row>
    </Frame>
  )
}

export function RemoteAccess({
  api,
  screen,
  daemon,
}: {
  api: ApiClient
  screen: ScreenDto
  daemon: RunningDaemon
}) {
  const read = useRemoteAccess(api)
  const turnOn = useTurnOnRemoteAccess(api)
  const turnOff = useTurnOffRemoteAccess(api)
  const restart = useDaemonRestart(api, daemon)
  // The switch holds what the Administrator asks for until they confirm
  // or cancel; the saved state is what the daemon answered.
  const [asking, setAsking] = useState<boolean | null>(null)
  // A turn-on that this page started, so its end asks for the restart.
  const [started, setStarted] = useState(false)
  const remote = read.data

  // The view loads its settings before it draws the switch, so the
  // browser's own scroll to the anchor finds nothing. The switch scrolls
  // itself into view once it is drawn.
  const drawn = remote !== undefined
  useEffect(() => {
    if (drawn && window.location.hash === `#${REMOTE_ACCESS_ANCHOR}`) {
      document.getElementById(REMOTE_ACCESS_ANCHOR)?.scrollIntoView()
    }
  }, [drawn])

  // The turn-on runs in the daemon. When it ends with Remote Access on,
  // the restart puts it in effect, as "Turn on and restart" said.
  const turnedOn = started && remote !== undefined && !remote.turning_on
  useEffect(() => {
    if (!turnedOn) return
    setStarted(false)
    if (remote.enabled && remote.restart_required) restart.ask()
  }, [turnedOn, remote, restart])

  if (remote === undefined) {
    return (
      <Frame id={REMOTE_ACCESS_ANCHOR}>
        <Row>
          <span className="system-row-note">
            {read.isError ? 'Remote Access could not be read.' : 'Reading…'}
          </span>
        </Row>
      </Frame>
    )
  }
  if (!remote.switchable) return <ServerRemoteAccess remote={remote} screen={screen} />

  const waiting = remote.turning_on ?? null
  const askOn = () =>
    turnOn.mutate(undefined, {
      onSuccess: () => {
        setAsking(null)
        setStarted(true)
      },
    })
  const askOff = () =>
    turnOff.mutate(undefined, {
      onSuccess: (saved) => {
        setAsking(null)
        setStarted(false)
        if (saved.restart_required) restart.ask()
      },
    })

  const busy = turnOn.isPending || turnOff.isPending || restart.isPending
  const checked = waiting !== null || (asking ?? remote.enabled)
  const switchable =
    !busy && waiting === null && (remote.enabled || asking !== null || canTurnOn(remote.tailscale))
  const failed = turnOn.isError ? turnOn.error : turnOff.isError ? turnOff.error : null
  const status = restartMessage(restart.phase, daemon.supervised)
  const ports = funnelPorts(remote.tailscale)
  const servesPagis = ports.length > 0 && ports.every((port) => port.served.serves === 'pagis')
  // The ports of the Funnel that still serve Pagis after a turn-off.
  const leftOn = ports.filter((port) => port.served.serves === 'pagis')
  // Remote Access is on and running, but the Funnel does not serve Pagis
  // on both ports.
  const broken = remote.enabled && !remote.restart_required && !servesPagis

  return (
    <Frame id={REMOTE_ACCESS_ANCHOR}>
      <Row>
        <Switch
          checked={checked}
          disabled={!switchable}
          onCheckedChange={(next) => {
            turnOn.reset()
            turnOff.reset()
            setAsking(next === remote.enabled ? null : next)
          }}
        >
          Remote Access
        </Switch>
        <span className="system-row-note">
          {remote.enabled && remote.public_origin ? (
            <>
              Other machines reach Pagis at{' '}
              <a href={remote.public_origin} target="_blank" rel="noreferrer">
                {remote.public_origin}
              </a>
              . They sign in with a sign-in link.
            </>
          ) : (
            'Only this computer reaches Pagis.'
          )}
        </span>
      </Row>

      {asking === null && waiting === null && remote.tailscale && (!remote.enabled || broken) && (
        <>
          {broken && (
            <Row>
              <span role="alert" className="system-row-note system-error">
                Tailscale Funnel does not serve Pagis now, so other machines do not reach it.
              </span>
              {canTurnOn(remote.tailscale) && (
                <Button disabled={busy} onClick={askOn}>
                  Turn on again
                </Button>
              )}
            </Row>
          )}
          {!(remote.tailscale.state === 'ready' && servesPagis) && (
            <TailscaleNote
              tailscale={remote.tailscale}
              checking={read.isFetching}
              onCheckAgain={() => void read.refetch()}
            />
          )}
          {!remote.enabled &&
            leftOn.map((port) => (
              <Row key={port.key}>
                <span className="system-row-note">
                  Tailscale Funnel still serves Pagis on port {port.number}, and Pagis refuses
                  other machines while Remote Access is off. To remove it, run{' '}
                  <code>{port.off}</code>.
                </span>
              </Row>
            ))}
        </>
      )}

      {remote.enabled && remote.restart_required && asking === null && waiting === null && (
        <Row>
          <span className="system-row-note">Remote Access takes effect when Pagis restarts.</span>
          <Button disabled={busy} onClick={() => restart.ask()}>
            Restart now
          </Button>
        </Row>
      )}

      {remote.enabled && asking === null && waiting === null && <ScreenNote screen={screen} />}

      {asking === true && (
        <Row>
          <span className="system-row-note">
            Pagis turns on Tailscale Funnel for port 443 of this computer, to Pagis on this
            computer, and for port 8443, to the TURN server that carries the live screen.{' '}
            {remote.tailscale?.state === 'ready' ? (
              <>
                Anyone on the internet then reaches the sign-in page at{' '}
                <span className="system-mono">https://{remote.tailscale.dns_name}</span>.
              </>
            ) : (
              <>Anyone on the internet then reaches the sign-in page at the name of this computer.</>
            )}{' '}
            Another machine signs in with a sign-in link, never with a password. Then Pagis
            restarts.{' '}
            <a href={REMOTE_ACCESS_GUIDE} target="_blank" rel="noreferrer">
              Remote Access
            </a>
          </span>
        </Row>
      )}

      {waiting !== null && (
        <Row>
          <span className="system-row-note" role="status">
            {waiting.enable_url ? (
              <>
                Tailscale asks you to turn on HTTPS and Funnel for your tailnet. Open{' '}
                <a href={waiting.enable_url} target="_blank" rel="noreferrer">
                  {waiting.enable_url}
                </a>{' '}
                and approve. Pagis goes on when you approve.
              </>
            ) : (
              'Tailscale turns on Funnel…'
            )}
          </span>
        </Row>
      )}

      {asking === false && (
        <Row>
          <span className="system-row-note">
            Other machines no longer reach Pagis. Pagis removes the Funnel and restarts. People who
            signed in from other machines keep their accounts and Sessions, and reach Pagis again
            when Remote Access is on.
          </span>
        </Row>
      )}

      {(asking !== null || waiting !== null || failed || remote.failure || status) && (
        <Row>
          {asking === true && (
            <Button variant="primary" disabled={busy} onClick={askOn}>
              Turn on and restart
            </Button>
          )}
          {asking === false && (
            <Button variant="primary" disabled={busy} onClick={askOff}>
              Turn off and restart
            </Button>
          )}
          {asking !== null && (
            <Button variant="ghost" disabled={busy} onClick={() => setAsking(null)}>
              Cancel
            </Button>
          )}
          {waiting !== null && (
            <Button variant="ghost" disabled={busy} onClick={askOff}>
              Cancel
            </Button>
          )}
          <span className="system-row-trailing system-row-note">
            {failed ? (
              <span role="alert" className="system-error">
                {errorMessage(failed, 'Remote Access could not be changed.')}
              </span>
            ) : remote.failure && waiting === null && asking === null ? (
              <span role="alert" className="system-error">
                {remote.failure}
              </span>
            ) : status ? (
              <span role="status">{status}</span>
            ) : null}
          </span>
        </Row>
      )}
    </Frame>
  )
}
