// The People section: the roster of the installation, the form
// that makes an account, and per person a monthly spend cap, a password
// reset and a disable switch.
//
// Accounts are administrator-created with a password: there is no
// self-signup. It is a view of the Administration Interface, and the
// routes behind it answer on the administration port alone.

import { useState } from 'react'

import type { ApiClient, PersonDto, PersonUsageDto } from '../../api/client'
import { Badge, Button, Frame, Input, Row, SectionLabel } from '../../primitives'
import {
  errorMessage,
  useCreateAccount,
  useInstallationUsage,
  usePeople,
  useResetAccountPassword,
  useSetAccountEnabled,
  useSetSignIn,
  useSetSpendCap,
} from '../../queries'

import './People.css'

/** The floor the daemon enforces, stated before the person types. */
const MIN_PASSWORD_LENGTH = 12

/** US dollars, to the cent. */
export function money(amount: number): string {
  return `$${amount.toFixed(2)}`
}

/** What a person is called on the roster: their name, else the address
 *  they sign in with. */
export function personLabel(person: PersonDto): string {
  return person.name ?? person.email ?? 'This person'
}

/** The form that makes an account. The daemon writes the person's
 *  Workspace with the same seed a first run uses, so they sign in to a
 *  sprite that can already think on the installation's keys. */
function NewAccount({ api }: { api: ApiClient }) {
  const create = useCreateAccount(api)
  const [email, setEmail] = useState('')
  const [name, setName] = useState('')
  const [password, setPassword] = useState('')
  const complete =
    email.includes('@') && name.trim() !== '' && password.length >= MIN_PASSWORD_LENGTH

  return (
    <Frame
      hint={`The person signs in with this address and password. A password is at least ${MIN_PASSWORD_LENGTH} characters, and they can change it later.`}
    >
      <Row className="people-new">
        <Input
          aria-label="Email address"
          placeholder="grace@example.com"
          value={email}
          onChange={(event) => setEmail(event.target.value)}
        />
        <Input
          aria-label="Name"
          placeholder="Grace"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
        <Input
          type="password"
          aria-label="First password"
          placeholder="First password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
        />
        <Button
          variant="primary"
          disabled={!complete || create.isPending}
          onClick={() =>
            create.mutate(
              { email: email.trim(), name: name.trim(), password },
              {
                onSuccess: () => {
                  setEmail('')
                  setName('')
                  setPassword('')
                },
              },
            )
          }
        >
          Create account
        </Button>
      </Row>
      {create.isError && (
        <span className="settings-error" role="alert">
          {errorMessage(create.error, 'That account could not be created.')}
        </span>
      )}
    </Frame>
  )
}

/** The address and password a person with neither signs in with.
 *
 *  The seeded person of a local installation holds neither: the client
 *  trades the Client Credential, so nothing asks them for a password
 *  until they want a browser to sign in without the client. This is
 *  where they set one, and it is also how an administrator gives a
 *  seeded person a way in on a server. */
function SetSignIn({ api, person }: { api: ApiClient; person: PersonDto }) {
  const set = useSetSignIn(api)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const label = personLabel(person)
  const ready = email.includes('@') && password.length >= MIN_PASSWORD_LENGTH

  return (
    <Row className="people-controls">
      <Input
        className="people-email-field"
        aria-label={`Email address for ${label}`}
        placeholder="grace@example.com"
        value={email}
        onChange={(event) => setEmail(event.target.value)}
      />
      <Input
        type="password"
        className="people-password"
        aria-label={`First password for ${label}`}
        placeholder="Password"
        value={password}
        onChange={(event) => setPassword(event.target.value)}
      />
      <Button
        size="sm"
        aria-label={`Set a way in for ${label}`}
        disabled={!ready || set.isPending}
        onClick={() =>
          set.mutate(
            { userId: person.id, email: email.trim(), password },
            { onSuccess: () => { setEmail(''); setPassword('') } },
          )
        }
      >
        Set a way in
      </Button>
      <span className="people-unit">
        {set.isError
          ? errorMessage(set.error, 'That way in could not be set.')
          : 'A browser signs in with these; the client needs neither.'}
      </span>
    </Row>
  )
}

/** One person: what they spent this month, their cap, a reset and the
 *  disable switch. */
function PersonRow({
  api,
  person,
  spend,
}: {
  api: ApiClient
  person: PersonDto
  spend: PersonUsageDto | undefined
}) {
  const setEnabled = useSetAccountEnabled(api)
  const reset = useResetAccountPassword(api)
  const setCap = useSetSpendCap(api)
  const [cap, setCap_] = useState(
    person.monthly_spend_cap_usd == null ? '' : String(person.monthly_spend_cap_usd),
  )
  const [password, setPassword] = useState('')
  const label = personLabel(person)
  const noCap = cap.trim() === ''
  const parsedCap = Number(cap)
  const capValid = noCap || (Number.isFinite(parsedCap) && parsedCap > 0)

  return (
    <div className="people-row">
      <Row>
        <span className="people-name">{label}</span>
        <span className="people-email">{person.email ?? 'Signs in on this machine'}</span>
        {person.role === 'administrator' && <Badge tone="working">Administrator</Badge>}
        {person.disabled && <Badge tone="failed">Disabled</Badge>}
        {spend?.cap_reached && <Badge tone="failed">At their cap</Badge>}
        <span className="people-spend">
          {money(spend?.total.cost_usd ?? 0)} this month
        </span>
        <Button
          size="sm"
          className="people-trailing"
          aria-label={
            person.disabled ? `Enable the account of ${label}` : `Disable the account of ${label}`
          }
          disabled={setEnabled.isPending}
          onClick={() => setEnabled.mutate({ userId: person.id, enabled: person.disabled })}
        >
          {person.disabled ? 'Enable' : 'Disable'}
        </Button>
      </Row>
      <Row className="people-controls">
        <Input
          type="number"
          min={1}
          step="0.01"
          className="people-cap"
          aria-label={`Monthly spend cap for ${label}`}
          placeholder="No cap"
          value={cap}
          onChange={(event) => setCap_(event.target.value)}
        />
        <span className="people-unit">per month</span>
        <Button
          size="sm"
          aria-label={`Save the spend cap for ${label}`}
          disabled={!capValid || setCap.isPending}
          onClick={() =>
            setCap.mutate({ userId: person.id, capUsd: noCap ? null : parsedCap })
          }
        >
          Save cap
        </Button>
        <Input
          type="password"
          className="people-password"
          aria-label={`New password for ${label}`}
          placeholder="New password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
        />
        <Button
          size="sm"
          aria-label={`Reset the password of ${label}`}
          disabled={password.length < MIN_PASSWORD_LENGTH || reset.isPending}
          onClick={() =>
            reset.mutate(
              { userId: person.id, password },
              { onSuccess: () => setPassword('') },
            )
          }
        >
          Reset password
        </Button>
      </Row>
      {person.email === null && <SetSignIn api={api} person={person} />}
      {(setEnabled.isError || reset.isError || setCap.isError) && (
        <span className="settings-error" role="alert">
          {errorMessage(
            setEnabled.error ?? reset.error ?? setCap.error,
            'That change could not be saved.',
          )}
        </span>
      )}
    </div>
  )
}

export function People({ api }: { api: ApiClient }) {
  const people = usePeople(api)
  const usage = useInstallationUsage(api)
  const spendOf = (userId: string) =>
    usage.data?.items.find((item) => item.person.id === userId)

  return (
    <section className="settings-section people">
      <div className="people-title">
        <h1>People</h1>
        <span>
          Who this installation serves. You create every account; there is no sign-up.
        </span>
      </div>
      <NewAccount api={api} />
      <SectionLabel>Roster</SectionLabel>
      <Frame
        hint="A disabled account signs in to nothing and keeps everything it owns, so enabling it again gives the same person their sprites and memory back. A person at their cap is told so in their conversation."
      >
        {(people.data ?? []).map((person) => (
          <PersonRow
            key={person.id}
            api={api}
            person={person}
            spend={spendOf(person.id)}
          />
        ))}
      </Frame>
      {usage.data && (
        <span className="people-total">
          {money(usage.data.total.cost_usd)} spent on model calls this month, across
          everybody.
        </span>
      )}
    </section>
  )
}
