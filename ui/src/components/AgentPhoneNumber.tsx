// The Agent Phone Number section of an Agent's own settings page
// (ADR-0018).
//
// Provisioning lives here and not on a tab of its own, because a number
// is the Agent's identity: to hold it is the authority to call, so
// there is no grant to give. Buying is three steps — search, confirm,
// assigned — and the card carries assign, unassign and release.
//
// Unassign and release are different acts, and the card says so. An
// unassigned number stays with the workspace and stays paid for; a
// released number goes back to the carrier and never comes back, so its
// confirmation names the Agent that loses the line.
//
// A number the account already holds is adopted, not bought:
// the user types it, the daemon proves it at the carrier, and the
// record lands like a purchase does.

import { useState } from 'react'

import type { AgentDto, ApiClient } from '../api/client'
import { Button, Input, Textarea } from '../primitives'
import {
  errorMessage,
  useAdoptPhoneNumber,
  useAssignPhoneNumber,
  useAvailableNumbers,
  useBuyPhoneNumber,
  usePhoneNumbers,
  useReleasePhoneNumber,
  useUnassignPhoneNumber,
  useUpdateAgent,
} from '../queries'

import { AdministratorSetsUp } from './settings/AdministratorSetsUp'
import './AgentPhoneNumber.css'
import './agent.css'
import './settings.css'

/** The line the card states without condition, because the refusal is a
 *  Pagis rule and not a carrier setting (ADR-0018). */
const EMERGENCY_NOTICE =
  'This line does not reach emergency services. Pagis refuses to call an emergency number, always.'

export type PhoneNumber = {
  id: string
  e164: string
  status: string
  agent_id?: string | null
  registration?: string | null
  registration_failure?: string | null
}

/** Where the line stands with the carrier (ADR-0020), shown the
 *  way a Connection shows `reauth_required`: an unregistered number
 *  drops inbound calls in silence, so the user must see it. */
const REGISTRATION_LABEL: Record<string, string> = {
  unregistered: 'Not registered',
  registering: 'Registering…',
  registered: 'Registered with the carrier',
  failed: 'Not registered',
}

/** Why the line is not registered, in the user's words. The
 *  agent page names the act, not the protocol. The carrier sign-in is
 *  the installation's, so the two failures it causes say who fixes
 *  them. */
const REGISTRATION_FAILURE: Record<string, string> = {
  no_credential: 'The carrier has no sign-in yet.',
  unauthorized: 'The carrier refused the sign-in.',
  unreachable: 'The carrier did not answer. Pagis keeps trying.',
  refused: 'The carrier refused the registration. Pagis keeps trying.',
}

/** The failures the installation's setup causes, which an
 *  administrator repairs. */
const SETUP_FAILURES = ['no_credential', 'unauthorized']

function RegistrationChip({ number }: { number: PhoneNumber }) {
  const registration = number.registration ?? null
  if (registration === null) return null
  const failed = registration === 'failed' || registration === 'unregistered'
  return (
    <span
      className={failed ? 'phone-chip phone-chip-warning' : 'phone-chip'}
      data-testid="phone-registration"
    >
      {REGISTRATION_LABEL[registration] ?? registration}
    </span>
  )
}

/** `+14155550123` reads as `+1 415 555 0123`. */
export function formatE164(e164: string): string {
  if (!/^\+1\d{10}$/.test(e164)) return e164
  return `+1 ${e164.slice(2, 5)} ${e164.slice(5, 8)} ${e164.slice(8)}`
}

function BuyNumber({
  api,
  agent,
  onDone,
}: {
  api: ApiClient
  agent: AgentDto
  onDone: () => void
}) {
  const [country, setCountry] = useState('US')
  const [areaCode, setAreaCode] = useState('')
  const [search, setSearch] = useState<{
    country: string
    areaCode: string
    locality: string
  } | null>(null)
  const [picked, setPicked] = useState<string | null>(null)
  const [bought, setBought] = useState<string | null>(null)
  const available = useAvailableNumbers(api, search)
  const buy = useBuyPhoneNumber(api)
  const step = bought !== null ? 3 : picked !== null ? 2 : 1

  return (
    <div className="phone-buy" aria-label={`Buy a number for ${agent.name}`}>
      <ol className="phone-steps">
        <li aria-current={step === 1 ? 'step' : undefined}>1 · Search</li>
        <li aria-current={step === 2 ? 'step' : undefined}>2 · Confirm</li>
        <li aria-current={step === 3 ? 'step' : undefined}>3 · Assigned</li>
      </ol>

      {step === 1 && (
        <section>
          <Input
            aria-label="Country"
            placeholder="Country, e.g. US"
            value={country}
            onChange={(event) => setCountry(event.target.value)}
          />
          <Input
            aria-label="Area code or city"
            placeholder="Area code or city, e.g. 415"
            value={areaCode}
            onChange={(event) => setAreaCode(event.target.value)}
          />
          <Button
            variant="primary"
            disabled={country.trim() === ''}
            onClick={() =>
              setSearch({
                country: country.trim(),
                areaCode: /^\d+$/.test(areaCode.trim()) ? areaCode.trim() : '',
                locality: /^\d+$/.test(areaCode.trim()) ? '' : areaCode.trim(),
              })
            }
          >
            Search
          </Button>
          <p className="settings-hint">
            Numbers come from your own carrier account. Pagis holds no pool
            of its own, and it stores no price: this is the carrier's price
            today.
          </p>
          {available.isError && (
            <p className="settings-error" role="alert">
              {errorMessage(available.error, 'The carrier did not answer.')}
            </p>
          )}
          {available.data?.length === 0 && <p>No numbers there.</p>}
          {(available.data ?? []).map((number) => (
            <Button
              key={number.e164}
              className="phone-offer"
              onClick={() => setPicked(number.e164)}
            >
              <strong>{formatE164(number.e164)}</strong>
              <span className="settings-hint">
                {[number.region, number.monthly_cost && `${number.monthly_cost} ${number.currency ?? ''} a month`]
                  .filter(Boolean)
                  .join(' · ')}
              </span>
            </Button>
          ))}
        </section>
      )}

      {step === 2 && picked !== null && (
        <section>
          <p>
            <strong>{formatE164(picked)}</strong> becomes {agent.name}'s desk
            line as soon as the carrier sells it.
          </p>
          <p className="settings-hint">{EMERGENCY_NOTICE}</p>
          {buy.isError && (
            <p className="settings-error" role="alert">
              {errorMessage(buy.error, 'The carrier did not sell that number.')}
            </p>
          )}
          <div className="settings-row-actions">
            <Button onClick={() => setPicked(null)}>Back</Button>
            <Button
              variant="primary"
              disabled={buy.isPending}
              onClick={() =>
                buy.mutate(
                  { e164: picked, agent_id: agent.id },
                  { onSuccess: (number) => setBought(number.e164) },
                )
              }
            >
              {buy.isPending ? 'Buying…' : 'Buy and assign'}
            </Button>
          </div>
        </section>
      )}

      {step === 3 && bought !== null && (
        <section aria-live="polite">
          <p>
            <strong>{formatE164(bought)}</strong> is {agent.name}'s desk line.
          </p>
          <Button variant="primary" onClick={onDone}>Done</Button>
        </section>
      )}
    </div>
  )
}

/** Add a number the carrier account already holds. With an
 *  Agent, the line is the Agent's at once; without one, the workspace
 *  holds it spare. */
export function AdoptNumber({
  api,
  agent,
  onDone,
}: {
  api: ApiClient
  agent: AgentDto | null
  onDone: () => void
}) {
  const adopt = useAdoptPhoneNumber(api)
  const [e164, setE164] = useState('')

  return (
    <div className="phone-adopt" aria-label="Add a number you own">
      <p className="settings-hint">
        A number already on your carrier account. Pagis checks the
        account holds it and buys nothing.
      </p>
      <Input
        aria-label="Phone number"
        placeholder="Phone number, e.g. +14155550123"
        value={e164}
        onChange={(event) => setE164(event.target.value)}
      />
      {adopt.isError && (
        <p className="settings-error" role="alert">
          {errorMessage(adopt.error, 'That number could not be added.')}
        </p>
      )}
      <div className="settings-row-actions">
        <Button
          variant="primary"
          disabled={adopt.isPending || e164.trim() === ''}
          onClick={() =>
            adopt.mutate(
              { e164: e164.trim(), agent_id: agent?.id },
              { onSuccess: onDone },
            )
          }
        >
          {agent === null ? 'Add number' : `Add to ${agent.name}`}
        </Button>
        <Button onClick={onDone}>Cancel</Button>
      </div>
    </div>
  )
}

export function ReleaseConfirmation({
  number,
  holder,
  pending,
  onCancel,
  onRelease,
}: {
  number: PhoneNumber
  holder: string | null
  pending: boolean
  onCancel: () => void
  onRelease: () => void
}) {
  return (
    <div className="phone-release" role="dialog" aria-label="Release number">
      <p>
        {formatE164(number.e164)} goes back to the carrier. The charge stops
        and the number never comes back. Past calls keep pointing at it.
      </p>
      {holder !== null && (
        <p className="settings-warning">
          {holder} holds this number and will have no line.
        </p>
      )}
      <div className="settings-row-actions">
        <Button onClick={onCancel}>Keep it</Button>
        <Button variant="danger" disabled={pending} onClick={onRelease}>
          Release it
        </Button>
      </div>
    </div>
  )
}

/** The Standing Brief (ADR-0020): what a call the Agent answers on
 *  this line is for. It sits on the desk-line card and not on the
 *  profile form, because an Agent with no line never answers a call.
 *
 *  Empty is a real answer and the common one: the Agent then takes a
 *  message. The daemon owns those words, so the hint says what happens
 *  and does not repeat them. */
function StandingBrief({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const update = useUpdateAgent(api)
  const saved = agent.standing_brief ?? ''
  const [brief, setBrief] = useState(saved)
  const changed = brief.trim() !== saved.trim()

  return (
    <div className="phone-brief">
      <label className="settings-label" htmlFor="standing-brief">
        What a call to this line is for
      </label>
      <p className="settings-hint">
        {agent.name} reads this at the start of every call it answers.
        Leave it empty and it takes a message: who called, what they
        need, and how to reach you.
      </p>
      <Textarea
        id="standing-brief"
        aria-label={`What a call to ${agent.name}'s line is for`}
        rows={4}
        value={brief}
        placeholder="For example: say I am in meetings until the evening, and ask what the call is about."
        onChange={(e) => setBrief(e.target.value)}
      />
      {update.error != null && (
        <p className="settings-error" role="alert">
          {errorMessage(update.error, 'That brief could not be saved.')}
        </p>
      )}
      <div className="settings-row-actions">
        <Button
          variant="primary"
          disabled={!changed || update.isPending}
          onClick={() =>
            update.mutate({
              agentId: agent.id,
              name: agent.name,
              job: agent.job,
              description: agent.description,
              personality: agent.personality,
              voice: agent.voice ?? null,
              standing_brief: brief.trim() === '' ? null : brief,
            })
          }
        >
          {update.isPending ? 'Saving…' : 'Save the brief'}
        </Button>
        {changed && !update.isPending && (
          <Button type="button" onClick={() => setBrief(saved)}>
            Cancel
          </Button>
        )}
      </div>
    </div>
  )
}

export function AgentPhoneNumber({
  api,
  agent,
}: {
  api: ApiClient
  agent: AgentDto
}) {
  const page = usePhoneNumbers(api)
  const assign = useAssignPhoneNumber(api)
  const unassign = useUnassignPhoneNumber(api)
  const release = useReleasePhoneNumber(api)
  const [buying, setBuying] = useState(false)
  const [adopting, setAdopting] = useState(false)
  const [releasing, setReleasing] = useState<string | null>(null)

  const numbers: PhoneNumber[] = page.data?.items ?? []
  const held = numbers.find((number) => number.agent_id === agent.id) ?? null
  const spare = numbers.filter((number) => number.status === 'unassigned')
  const carrier = page.data?.carrier ?? null
  const canBuy = carrier !== null && carrier.status === 'connected'
  const failure =
    assign.error ?? unassign.error ?? release.error ?? null

  return (
    <section className="agent-phone" aria-label={`${agent.name} phone number`}>
      <div className="agent-section-header">
        <h4>Phone number</h4>
        {held === null && !buying && !adopting && canBuy && (
          <div className="settings-row-actions">
            <Button onClick={() => setBuying(true)}>Buy a number</Button>
            <Button onClick={() => setAdopting(true)}>Add a number you own</Button>
          </div>
        )}
      </div>
      <p className="settings-hint">
        {agent.name} holds one number: its desk line. To hold a number is
        the authority to call, so there is no separate grant.
      </p>

      {carrier === null && (
        <AdministratorSetsUp api={api} testId="no-carrier">
          This installation has no phone carrier yet, so no number can be
          bought.
        </AdministratorSetsUp>
      )}
      {carrier !== null && carrier.status !== 'connected' && (
        <AdministratorSetsUp api={api} tone="warning" testId="carrier-needs-repair">
          {carrier.display_name} is {carrier.status.replace('_', ' ')}, so no
          number can be bought until it is fixed.
        </AdministratorSetsUp>
      )}

      {failure !== null && (
        <p className="settings-error" role="alert">
          {errorMessage(failure, 'That could not be done.')}
        </p>
      )}

      {held !== null && (
        <div className="phone-card" data-testid="agent-phone-card">
          <div className="phone-card-head">
            <strong>{formatE164(held.e164)}</strong>
            <span className="phone-chips">
              <span className="phone-chip">Assigned to {agent.name}</span>
              <RegistrationChip number={held} />
            </span>
          </div>
          {held.registration_failure != null &&
            (SETUP_FAILURES.includes(held.registration_failure) ? (
              <div role="alert">
                <AdministratorSetsUp api={api} tone="warning">
                  {REGISTRATION_FAILURE[held.registration_failure]}
                </AdministratorSetsUp>
              </div>
            ) : (
              <p className="settings-warning" role="alert">
                {REGISTRATION_FAILURE[held.registration_failure] ??
                  `Registration failed: ${held.registration_failure}.`}
              </p>
            ))}
          <p className="settings-warning">{EMERGENCY_NOTICE}</p>
          <StandingBrief api={api} agent={agent} />
          <div className="settings-row-actions">
            <Button
              aria-label={`Unassign ${held.e164} from ${agent.name}`}
              disabled={unassign.isPending}
              onClick={() => unassign.mutate(held.id)}
            >
              Unassign from {agent.name}
            </Button>
            <Button
              variant="danger"
              aria-label={`Release ${held.e164}`}
              onClick={() => setReleasing(held.id)}
            >
              Release to the carrier
            </Button>
          </div>
          <p className="settings-hint">
            Unassigned stays with the workspace and stays paid for.
            Released goes back to the carrier for good.
          </p>
          {releasing === held.id && (
            <ReleaseConfirmation
              number={held}
              holder={agent.name}
              pending={release.isPending}
              onCancel={() => setReleasing(null)}
              onRelease={() =>
                release.mutate(held.id, { onSuccess: () => setReleasing(null) })
              }
            />
          )}
        </div>
      )}

      {held === null &&
        spare.map((number) => (
          <div className="phone-card" key={number.id} data-testid="agent-phone-card">
            <div className="phone-card-head">
              <strong>{formatE164(number.e164)}</strong>
              <span className="phone-chip">Unassigned</span>
            </div>
            <div className="settings-row-actions">
              <Button
                variant="primary"
                aria-label={`Assign ${number.e164} to ${agent.name}`}
                disabled={assign.isPending}
                onClick={() =>
                  assign.mutate({ phoneNumberId: number.id, agentId: agent.id })
                }
              >
                Assign to {agent.name}
              </Button>
              <Button
                variant="danger"
                aria-label={`Release ${number.e164}`}
                onClick={() => setReleasing(number.id)}
              >
                Release to the carrier
              </Button>
            </div>
            {releasing === number.id && (
              <ReleaseConfirmation
                number={number}
                holder={null}
                pending={release.isPending}
                onCancel={() => setReleasing(null)}
                onRelease={() =>
                  release.mutate(number.id, {
                    onSuccess: () => setReleasing(null),
                  })
                }
              />
            )}
          </div>
        ))}

      {buying && (
        <BuyNumber api={api} agent={agent} onDone={() => setBuying(false)} />
      )}
      {adopting && (
        <AdoptNumber api={api} agent={agent} onDone={() => setAdopting(false)} />
      )}
    </section>
  )
}
