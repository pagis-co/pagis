// The Home Exit (ADR-0029): the one computer of the Person through which
// their Agents' Computers on a server reach the internet, so that sites
// see the Person's own connection and not a data-center address. The copy
// calls an Agent a sprite, as the Product App does.
//
// The Person chooses one of their own Hosts that declared `exit`: a
// computer that runs the Pagis Client App connected to this server.
// Before they turn it on, the card states the five costs of the Home
// Exit. A change switches each awake Computer at once, and the card names
// each one that did not switch. A local installation has no Home Exit,
// so the card shows nothing there. When the Administrator turned the
// Home Exit off for the server, the card says so and keeps the choice.

import { useState } from 'react'

import type { ApiClient, HomeExitDto, HomeExitHostDto, SwitchedHomeExitDto } from '../../api/client'
import { Badge, Button, Frame, Row, Select, SectionLabel, Switch } from '../../primitives'
import { useIsMobile } from '../../state/useIsMobile'
import {
  errorMessage,
  useChooseHomeExit,
  useHomeExit,
  useTurnOffHomeExit,
} from '../../queries'
import { SettingsSection } from './SettingsSection'

import './HomeExit.css'

/** The documentation page of the Home Exit. */
export const HOME_EXIT_GUIDE = 'https://docs.pagis.co/client-app/home-exit'

const TITLE = 'Home Exit'
const LEAD = 'The computer of yours that your sprites reach the internet through.'

/** What the Person accepts when `name` becomes their Home Exit, in the
 *  words of ADR-0029. */
function costs(name: string): string[] {
  return [
    `Sites see the internet address of ${name} for every page the sprites open.`,
    "A block or an abuse report lands on that address, and sites can link the sprites' accounts to the household's own accounts.",
    "Every byte that a sprite's computer loads crosses that connection twice, once in and once out, and its upload speed limits the pages.",
    'Some internet providers forbid a proxy service in their terms.',
    `The sprites reach nothing on ${name} or on its local network.`,
  ]
}

/** How the select names a Host. */
function choices(hosts: HomeExitHostDto[]) {
  return hosts.map((host) => ({
    value: host.id,
    label: host.present ? host.name : `${host.name} (not connected)`,
  }))
}

/** The Home Exit while the Person has none: why it exists, the costs of
 *  the Host in the select, and the turn-on. */
function Off({
  homeExit,
  busy,
  onTurnOn,
}: {
  homeExit: HomeExitDto
  busy: boolean
  onTurnOn: (hostId: string) => void
}) {
  const first = homeExit.hosts.find((host) => host.present) ?? homeExit.hosts[0]
  const [hostId, setHostId] = useState(first?.id)
  const selected = homeExit.hosts.find((host) => host.id === hostId) ?? first

  return (
    <>
      <Row>
        <span className="home-exit-note">
          Your sprites&apos; computers reach the internet from the server. Sites see a data-center
          address, and some sites block it or ask for more checks.
        </span>
      </Row>
      {selected === undefined ? (
        <Row>
          <span className="home-exit-note">
            No computer of yours can be the Home Exit yet. Open the Pagis Client App on a computer
            at home and connect it to this server, and it shows here.
          </span>
        </Row>
      ) : (
        <>
          <Row className="home-exit-costs">
            <span className="home-exit-note">When you turn on the Home Exit:</span>
            <ul className="home-exit-cost-list">
              {costs(selected.name).map((cost) => (
                <li key={cost}>{cost}</li>
              ))}
            </ul>
          </Row>
          <Row>
            <Select
              label={TITLE}
              value={selected.id}
              disabled={busy}
              onValueChange={setHostId}
              items={choices(homeExit.hosts)}
            />
            <Button variant="primary" disabled={busy} onClick={() => onTurnOn(selected.id)}>
              Turn on
            </Button>
          </Row>
        </>
      )}
    </>
  )
}

/** The Home Exit while it is on: which Host carries the connections, a
 *  change to another Host, and the turn-off. */
function On({
  homeExit,
  chosen,
  busy,
  onChoose,
  onTurnOff,
}: {
  homeExit: HomeExitDto
  chosen: HomeExitHostDto
  busy: boolean
  onChoose: (hostId: string) => void
  onTurnOff: () => void
}) {
  return (
    <>
      <Row>
        <Badge tone="accent">On</Badge>
        <span className="home-exit-note">
          Your sprites&apos; computers reach the internet through {chosen.name}.
          {!chosen.present &&
            ` ${chosen.name} is not connected now, so they reach the internet from the server.`}
        </span>
      </Row>
      <Row>
        {homeExit.hosts.length > 1 && (
          <Select
            label={TITLE}
            value={chosen.id}
            disabled={busy}
            onValueChange={onChoose}
            items={choices(homeExit.hosts)}
          />
        )}
        <Button variant="danger-quiet" disabled={busy} onClick={onTurnOff}>
          Turn off
        </Button>
      </Row>
    </>
  )
}

/** The Home Exit while the Administrator turned it off for the server. */
function TurnedOffByAdministrator({ chosen }: { chosen: HomeExitHostDto | null | undefined }) {
  return (
    <>
      <Row>
        <Badge tone="neutral">Off</Badge>
        <span className="home-exit-note">
          The Administrator turned off the Home Exit for this server, so your sprites&apos;
          computers reach the internet from the server.
        </span>
      </Row>
      {chosen && (
        <Row>
          <span className="home-exit-note">
            Your choice of {chosen.name} stays, and it is in effect again when the Administrator
            turns the Home Exit on.
          </span>
        </Row>
      )}
    </>
  )
}

/** The Computers that did not switch at the last change. */
function NotSwitched({ saved }: { saved: SwitchedHomeExitDto | undefined }) {
  if (saved === undefined || saved.not_switched.length === 0) return null
  return (
    <Row>
      <span role="alert" className="home-exit-note settings-error">
        {saved.not_switched.map((failure) => (
          <span key={failure.agent_id} className="home-exit-failure">
            {failure.agent_name}&apos;s computer did not switch: {failure.error}.{' '}
          </span>
        ))}
        It takes the change when it wakes again.
      </span>
    </Row>
  )
}

export function HomeExit({ api }: { api: ApiClient }) {
  const phone = useIsMobile()
  const read = useHomeExit(api)
  const choose = useChooseHomeExit(api)
  const turnOff = useTurnOffHomeExit(api)
  // The answer of the last change, which names the Computers that did
  // not switch. The card moves between its views on a change, so the
  // card and not a view keeps it.
  const [saved, setSaved] = useState<SwitchedHomeExitDto>()
  const homeExit = read.data

  if (read.isError) {
    return (
      <SettingsSection title={TITLE} lead={LEAD}>
        <Frame>
          <Row>
            <span className="settings-hint">The Home Exit could not be read.</span>
          </Row>
        </Frame>
      </SettingsSection>
    )
  }
  if (homeExit === undefined || !homeExit.available) return null

  const busy = choose.isPending || turnOff.isPending
  const failed = choose.isError ? choose.error : turnOff.isError ? turnOff.error : null
  const onChoose = (hostId: string) => {
    turnOff.reset()
    choose.mutate(hostId, { onSuccess: setSaved })
  }
  const onTurnOff = () => {
    choose.reset()
    turnOff.mutate(undefined, { onSuccess: setSaved })
  }

  if (phone) {
    const host = homeExit.chosen ?? homeExit.hosts.find((item) => item.present) ?? homeExit.hosts[0]
    return <section className="phone-section"><SectionLabel>Home Exit</SectionLabel><p className="phone-hint">{LEAD}</p><Frame>{host ? <Switch row checked={!!homeExit.chosen && !homeExit.administrator_turned_off} disabled={busy || homeExit.administrator_turned_off || !host.present} onCheckedChange={(enabled) => enabled ? onChoose(host.id) : onTurnOff()}><span className="phone-row-copy"><span>{host.name}</span><span className="phone-hint">{homeExit.administrator_turned_off ? 'The administrator turned Home Exit off.' : homeExit.chosen ? 'On. Your sprites reach the internet through it.' : 'Off. Your sprites reach the internet directly.'}</span></span></Switch> : <Row>No computer of yours can be the Home Exit yet.</Row>}<NotSwitched saved={saved} /></Frame>{failed && <p role="alert" className="phone-hint">{errorMessage(failed, 'The Home Exit could not be changed.')}</p>}<p className="phone-hint">When this computer sleeps, your sprites reach the internet directly until it is back.</p></section>
  }

  return (
    <SettingsSection
      title={TITLE}
      lead={LEAD}
      hint={
        <>
          When the computer sleeps or its Client App quits, your sprites&apos; computers reach the
          internet from the server until it is back.{' '}
          <a href={HOME_EXIT_GUIDE} target="_blank" rel="noreferrer">
            Home Exit
          </a>
        </>
      }
    >
      <Frame className="home-exit">
        {homeExit.administrator_turned_off ? (
          <TurnedOffByAdministrator chosen={homeExit.chosen} />
        ) : homeExit.chosen ? (
          <On
            homeExit={homeExit}
            chosen={homeExit.chosen}
            busy={busy}
            onChoose={onChoose}
            onTurnOff={onTurnOff}
          />
        ) : (
          <Off homeExit={homeExit} busy={busy} onTurnOn={onChoose} />
        )}
        {failed && (
          <Row>
            <span role="alert" className="settings-error">
              {errorMessage(failed, 'The Home Exit could not be changed.')}
            </span>
          </Row>
        )}
        <NotSwitched saved={saved} />
      </Frame>
    </SettingsSection>
  )
}
