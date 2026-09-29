// The multi-user mode (ADR-0024): one switch that shows whether People
// on other machines reach this installation, and turns it on and off.
//
// The mode follows from the Public Origin, so turning it on asks for the
// origin that the owner's proxy or tunnel answers on, and the address that
// proxy reaches Pagis from. Pagis provides no TLS; the owner's proxy or
// tunnel does. Either change takes effect on a restart, which the button
// asks for. A server always serves a network, so it shows the mode and
// no switch.

import { useEffect, useState } from 'react'

import type { ApiClient, MultiUserDto, ScreenDto } from '../../api/client'
import { Badge, Button, Frame, Input, Row, Switch } from '../../primitives'
import { errorMessage, useDisableMultiUser, useEnableMultiUser } from '../../queries'
import { type RunningDaemon, restartMessage, useDaemonRestart } from './restart'

/** The deployment document's section with the Caddy, Tailscale Serve and
 *  Cloudflare Tunnel setup for a local installation. */
export const SETUP_GUIDE =
  'https://github.com/pagis-co/pagis/blob/main/docs/DEPLOYING-A-SERVER.md#a-local-installation-that-serves-several-people'

/** The section that names the Media Relay setting of each proxy and
 *  tunnel, and the TURN variant for a tunnel that carries no UDP. */
export const SCREEN_GUIDE =
  'https://github.com/pagis-co/pagis/blob/main/docs/DEPLOYING-A-SERVER.md#the-live-screen-for-other-people'

/** The live screen goes to a browser over UDP at the Media Relay's
 *  address, not through the proxy or tunnel, so the mode alone does not
 *  carry it to another machine. The note names the address that the
 *  running daemon advertises and who reaches it. */
function ScreenNote({ screen }: { screen: ScreenDto }) {
  const guide = (
    <a href={SCREEN_GUIDE} target="_blank" rel="noreferrer">
      Set up the live screen for other people
    </a>
  )
  if (screen.relay === 'turn') {
    return (
      <Row>
        <span className="system-row-note">
          The live screen of a Computer goes through your TURN server and not through your
          proxy or tunnel. Machines that reach the TURN server reach the live screen. {guide}.
        </span>
      </Row>
    )
  }
  const address = <code>{screen.advertise_ip}</code>
  return (
    <Row>
      <span className="system-row-note">
        The live screen of a Computer does not go through your proxy or tunnel.{' '}
        {screen.loopback ? (
          <>
            The Media Relay advertises {address}, so only this computer reaches the live
            screen. To change that, set <code>[screen] advertise_ip</code> in config.toml to an
            address that other machines reach, and restart Pagis.
          </>
        ) : (
          <>
            The Media Relay advertises {address}, so machines that reach this address reach the
            live screen. The firewall must let UDP ports{' '}
            <code>
              {screen.media_port_first}–{screen.media_port_last}
            </code>{' '}
            through to this computer.
          </>
        )}{' '}
        {guide}.
      </span>
    </Row>
  )
}

/** The anchor of the switch in the Settings view. The Client App opens
 *  `/settings#multi-user` after a setup for several People. */
export const MULTI_USER_ANCHOR = 'multi-user'

/** A proxy or tunnel on this computer reaches Pagis from loopback. */
const SAME_MACHINE_PROXY = '127.0.0.1'

function Origin({ multiUser }: { multiUser: MultiUserDto }) {
  return (
    <>
      <Row>
        <span className="system-about-key">Public Origin</span>
        <span className="system-mono">{multiUser.public_origin}</span>
      </Row>
      <Row>
        <span className="system-about-key">Trusted Proxy</span>
        <span className="system-mono">{multiUser.trusted_proxy ?? 'None'}</span>
      </Row>
    </>
  )
}

export function MultiUser({
  api,
  multiUser,
  screen,
  daemon,
}: {
  api: ApiClient
  multiUser: MultiUserDto
  screen: ScreenDto
  daemon: RunningDaemon
}) {
  const enable = useEnableMultiUser(api)
  const disable = useDisableMultiUser(api)
  const restart = useDaemonRestart(api, daemon)
  // The switch holds what the Administrator asks for until they confirm
  // or cancel; the saved mode is what the daemon answered.
  const [asking, setAsking] = useState<boolean | null>(null)
  const [origin, setOrigin] = useState('')
  const [proxy, setProxy] = useState(SAME_MACHINE_PROXY)

  // The view loads its settings before it draws the switch, so the
  // browser's own scroll to the anchor finds nothing. The switch scrolls
  // itself into view once it is drawn.
  useEffect(() => {
    if (window.location.hash === `#${MULTI_USER_ANCHOR}`) {
      document.getElementById(MULTI_USER_ANCHOR)?.scrollIntoView()
    }
  }, [])

  if (!multiUser.switchable) {
    return (
      <Frame id={MULTI_USER_ANCHOR}>
        <Row>
          <Badge tone="accent">Multi-user</Badge>
          <span className="system-row-note">
            A server always serves several People. Its deployment names the Public Origin.
          </span>
        </Row>
        <Origin multiUser={multiUser} />
        <ScreenNote screen={screen} />
      </Frame>
    )
  }

  const onSaved = (saved: { restart_required: boolean }) => {
    setAsking(null)
    if (saved.restart_required) restart.ask()
  }
  const turnOn = () =>
    enable.mutate(
      {
        public_origin: origin.trim(),
        trusted_proxy: proxy.trim() === '' ? null : proxy.trim(),
      },
      { onSuccess: onSaved },
    )
  const turnOff = () => disable.mutate(undefined, { onSuccess: onSaved })

  const busy = enable.isPending || disable.isPending || restart.isPending
  const checked = asking ?? multiUser.enabled
  const failed = enable.isError ? enable.error : disable.isError ? disable.error : null
  const status =
    restartMessage(restart.phase, daemon.supervised) ??
    ((enable.isSuccess || disable.isSuccess) && asking === null ? 'Saved.' : null)

  return (
    <Frame id={MULTI_USER_ANCHOR}>
      <Row>
        <Switch
          checked={checked}
          onCheckedChange={(next) => {
            enable.reset()
            disable.reset()
            setAsking(next === multiUser.enabled ? null : next)
          }}
        >
          Multi-user mode
        </Switch>
        <span className="system-row-note">
          {multiUser.enabled
            ? 'People on other machines reach Pagis through your proxy or tunnel.'
            : 'Only this computer reaches Pagis.'}
        </span>
      </Row>

      {multiUser.enabled && asking === null && (
        <>
          <Origin multiUser={multiUser} />
          <ScreenNote screen={screen} />
        </>
      )}

      {asking === true && (
        <>
          <div className="system-form">
            <label>
              Public Origin
              <Input
                className="system-mono"
                placeholder="https://pagis.example.net"
                value={origin}
                onChange={(event) => setOrigin(event.target.value)}
              />
            </label>
            <label>
              Trusted Proxy
              <Input
                className="system-mono"
                placeholder="None"
                value={proxy}
                onChange={(event) => setProxy(event.target.value)}
              />
            </label>
          </div>
          <Row>
            <span className="system-row-note">
              Pagis does not provide TLS. Put your own proxy or tunnel on this computer in
              front of it, and type the address people open as the Public Origin. The Trusted
              Proxy is the address that proxy reaches Pagis from: {SAME_MACHINE_PROXY} on this
              computer. Set up{' '}
              <a href={SETUP_GUIDE} target="_blank" rel="noreferrer">
                Caddy, Tailscale Serve or Cloudflare Tunnel
              </a>
              .
            </span>
          </Row>
          <ScreenNote screen={screen} />
        </>
      )}

      {asking === false && (
        <Row>
          <span className="system-row-note">
            Only this computer reaches Pagis again. People who signed in from other machines
            keep their accounts, and they cannot reach Pagis until the mode is on again.
          </span>
        </Row>
      )}

      {(asking !== null || failed || status) && (
        <Row>
          {asking === true && (
            <Button
              variant="primary"
              disabled={busy || origin.trim() === ''}
              onClick={turnOn}
            >
              Turn on and restart
            </Button>
          )}
          {asking === false && (
            <Button variant="primary" disabled={busy} onClick={turnOff}>
              Turn off and restart
            </Button>
          )}
          {asking !== null && (
            <Button variant="ghost" disabled={busy} onClick={() => setAsking(null)}>
              Cancel
            </Button>
          )}
          <span className="system-row-trailing system-row-note">
            {failed ? (
              <span role="alert" className="system-error">
                {errorMessage(failed, 'The mode could not be changed.')}
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
