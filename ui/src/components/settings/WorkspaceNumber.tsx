// The Workspace Number in Settings uses the carrier search and purchase flow.
import { useState } from 'react'

import type { ApiClient, WorkspaceNumberDto } from '../../api/client'
import { formatE164 } from '../../blocks/call'
import { Button, Input } from '../../primitives'
import {
  errorMessage,
  useAgents,
  useAvailableNumbers,
  useBuyPhoneNumber,
  usePhoneNumbers,
  useReleasePhoneNumber,
  useWorkspace,
} from '../../queries'
import { AdministratorSetsUp } from './AdministratorSetsUp'
import { StandingBrief } from './StandingBrief'
import './WorkspaceNumber.css'

const EMERGENCY_NOTICE = 'This line does not reach emergency services.'

const REGISTRATION_LABEL: Record<string, string> = {
  unregistered: 'Not registered',
  registering: 'Registering…',
  registered: 'Registered with the carrier',
  failed: 'Not registered',
}

/** Why the line is not registered, in the user's words. The
 *  Settings names the act, not the protocol. The carrier sign-in is
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

function RegistrationChip({ number }: { number: WorkspaceNumberDto }) {
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

function BuyNumber({
  api,
  onDone,
  onBuying,
}: {
  api: ApiClient
  onDone: () => void
  onBuying: () => void
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
    <div className="phone-buy" aria-label="Buy a Workspace Number">
      <ol className="phone-steps">
        <li aria-current={step === 1 ? 'step' : undefined}>1 · Search</li>
        <li aria-current={step === 2 ? 'step' : undefined}>2 · Confirm</li>
        <li aria-current={step === 3 ? 'step' : undefined}>3 · Ready</li>
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
              onClick={() => {
                setPicked(number.e164)
                onBuying()
              }}
            >
              <strong>{formatE164(number.e164)}</strong>
              <span className="settings-hint">
                {[
                  number.region,
                  number.monthly_cost && `${number.monthly_cost} ${number.currency ?? ''} a month`,
                ]
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
            <strong>{formatE164(picked)}</strong> becomes the Workspace Number
            when the carrier sells it.
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
                  { e164: picked },
                  { onSuccess: (number) => setBought(number.e164) },
                )
              }
            >
              {buy.isPending ? 'Buying…' : 'Buy number'}
            </Button>
          </div>
        </section>
      )}

      {step === 3 && bought !== null && (
        <section aria-live="polite">
          <p>
            <strong>{formatE164(bought)}</strong> is the Workspace Number.
          </p>
          <Button variant="primary" onClick={onDone}>
            Done
          </Button>
        </section>
      )}
    </div>
  )
}

export function WorkspaceNumber({ api }: { api: ApiClient }) {
  const page = usePhoneNumbers(api)
  const agents = useAgents(api)
  const workspace = useWorkspace(api)
  const chief = agents.data?.find(
    (agent) => agent.id === workspace.data?.chief_of_staff_agent_id,
  )
  const release = useReleasePhoneNumber(api)
  const [buying, setBuying] = useState(false)
  const [releasing, setReleasing] = useState(false)
  const number = page.data?.number ?? null
  const carrier = page.data?.carrier ?? null
  const failure = page.error ?? release.error

  return (
    <section className="settings-section workspace-number" aria-label="Workspace Number">
      <h1>Workspace Number</h1>
      <p className="settings-hint">
        The Chief of Staff answers Calls and receives texts to this number.
      </p>
      {failure != null && (
        <p className="settings-error" role="alert">
          {errorMessage(failure, 'The number could not be changed.')}
        </p>
      )}
      {!page.isPending && carrier === null && (
        <AdministratorSetsUp api={api} testId="no-carrier">
          This installation has no phone carrier yet, so no number can be bought.
        </AdministratorSetsUp>
      )}
      {carrier !== null && carrier.status !== 'connected' && (
        <AdministratorSetsUp api={api} tone="warning" testId="carrier-needs-repair">
          {carrier.display_name} is {carrier.status.replace('_', ' ')}. Repair the
          carrier Connection to buy a number.
        </AdministratorSetsUp>
      )}
      {number !== null && !buying && (
        <div className="phone-card" data-testid="workspace-number-card">
          <div className="phone-card-head">
            <strong>{formatE164(number.e164)}</strong>
            <RegistrationChip number={number} />
          </div>
          {number.registration_failure != null &&
            (SETUP_FAILURES.includes(number.registration_failure) ? (
              <div role="alert">
                <AdministratorSetsUp api={api} tone="warning">
                  {REGISTRATION_FAILURE[number.registration_failure]}
                </AdministratorSetsUp>
              </div>
            ) : (
              <p className="settings-warning" role="alert">
                {REGISTRATION_FAILURE[number.registration_failure] ??
                  `Registration failed: ${number.registration_failure}.`}
              </p>
            ))}
          <p className="settings-warning">{EMERGENCY_NOTICE}</p>
          {chief && <StandingBrief key={chief.id} api={api} agent={chief} />}
          <div className="settings-row-actions">
            <Button variant="danger" onClick={() => setReleasing(true)}>
              Release number
            </Button>
          </div>
          {releasing && (
            <div className="phone-release" role="dialog" aria-label="Release number">
              <p>
                Release <strong>{formatE164(number.e164)}</strong> to the carrier?
                The Workspace loses this number. The charge stops.
              </p>
              <div className="settings-row-actions">
                <Button onClick={() => setReleasing(false)}>Keep it</Button>
                <Button
                  variant="danger"
                  disabled={release.isPending}
                  onClick={() =>
                    release.mutate(undefined, {
                      onSuccess: () => {
                        setReleasing(false)
                        setBuying(false)
                      },
                    })
                  }
                >
                  {release.isPending ? 'Releasing…' : 'Release it'}
                </Button>
              </div>
            </div>
          )}
        </div>
      )}
      {(number === null || buying) && carrier?.status === 'connected' && (
        <BuyNumber
          api={api}
          onBuying={() => setBuying(true)}
          onDone={() => setBuying(false)}
        />
      )}
    </section>
  )
}
