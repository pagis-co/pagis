// The Coding Harnesses of one machine, under its row in Settings › Hosts
// (ADR-0033): whether the machine can start each harness of the Harness
// Catalog, its sign-in state there, a Harness Sign-In for each of its
// methods, and how the last sign-in ended.
//
// The Product App only starts the sign-in and the check. The Person
// finishes the sign-in in the vendor's own program in a terminal window on
// that machine, and the Client App runs the vendor's own status command
// there, so no field here takes or shows a credential (ADR-0022).

import type { ApiClient, HarnessDto, HarnessSignInMethod, HostDto } from '../../api/client'
import { Badge, type BadgeTone, Button, Row } from '../../primitives'
import {
  errorMessage,
  useCheckHarnessSignIn,
  useHarnesses,
  useStartHarnessSignIn,
} from '../../queries'

import '../settings.css'
import './HostHarnesses.css'

/** The words of the button of each sign-in method. */
const SIGN_IN_BUTTONS: Record<HarnessSignInMethod, string> = {
  subscription: 'Sign in with a subscription',
  api_key: 'Sign in with an API key',
}

type HostHarness = HostDto['harnesses'][number]
type LastSignIn = NonNullable<HostHarness['last_sign_in']>

/**
 * The mark of the sign-in report, or null when Pagis knows nothing.
 *
 * A status command reads the stored credential and does not test it. So a
 * harness that refused a session while its status command says "signed
 * in" has a credential that expired or that the vendor revoked.
 */
function signInMark(report: HostHarness): { label: string; tone: BadgeTone } | null {
  if (report.sign_in_state === 'not_signed_in') return { label: 'Not signed in', tone: 'waiting' }
  if (report.needs_sign_in) {
    return report.sign_in_state === 'signed_in'
      ? { label: 'Sign-in expired', tone: 'waiting' }
      : { label: 'Needs sign-in', tone: 'waiting' }
  }
  if (report.sign_in_state === 'signed_in') return { label: 'Signed in', tone: 'neutral' }
  return null
}

/** The hint after the terminal window of a sign-in closed. */
function signInEnd(attempt: LastSignIn, report: HostHarness, harness: string, machine: string): string {
  if (attempt.exit_code === 0) {
    if (report.sign_in_state === 'signed_in') return `You are signed in to ${harness} on ${machine}.`
    if (report.sign_in_state === 'not_signed_in') {
      return `The sign-in ended, and ${harness} is still not signed in on ${machine}.`
    }
    return 'The sign-in ended. The next session tells whether it worked.'
  }
  if (attempt.exit_code !== null && attempt.exit_code !== undefined) {
    return `The sign-in ended with exit code ${attempt.exit_code}. Sign in again.`
  }
  const why = attempt.error ?? 'The sign-in did not end'
  return `${why.charAt(0).toUpperCase()}${why.slice(1)}. Sign in again.`
}

function HarnessRow({
  api,
  host,
  harness,
}: {
  api: ApiClient
  host: HostDto
  harness: HarnessDto
}) {
  const signIn = useStartHarnessSignIn(api)
  const check = useCheckHarnessSignIn(api)
  // The Host gives one entry for each `harness:<id>` capability, with
  // the report of the daemon.
  const declared = host.harnesses.find((entry) => entry.id === harness.id)
  const canStart = declared !== undefined
  const mark = declared === undefined ? null : signInMark(declared)
  // Only the sign-in that this page started shows how it ended.
  const attempt =
    signIn.isSuccess && declared?.last_sign_in?.id === signIn.data.id ? declared.last_sign_in : null
  const ended = attempt !== null && !attempt.running

  return (
    <Row
      role="group"
      aria-label={harness.name}
      className="host-harness"
      data-testid="host-harness"
    >
      <span className="host-harness-name">{harness.name}</span>
      <span className="host-capability">
        {canStart ? 'Can start' : 'Not found on this computer'}
      </span>
      {mark !== null && <Badge tone={mark.tone}>{mark.label}</Badge>}
      <div className="settings-row-actions host-harness-actions">
        {harness.checks_sign_in && (
          <Button
            size="sm"
            variant="ghost"
            disabled={!host.present || !canStart || check.isPending}
            onClick={() => check.mutate({ hostId: host.id, harnessId: harness.id })}
          >
            Check sign-in
          </Button>
        )}
        {harness.sign_in_methods.map(({ method }) => (
          <Button
            key={method}
            size="sm"
            disabled={!host.present || !canStart || signIn.isPending}
            onClick={() =>
              signIn.mutate({ hostId: host.id, harnessId: harness.id, method })
            }
          >
            {SIGN_IN_BUTTONS[method]}
          </Button>
        ))}
      </div>
      {signIn.isSuccess && (
        <p className="settings-hint host-harness-status">
          {ended && declared !== undefined
            ? signInEnd(attempt, declared, harness.name, host.name)
            : `A terminal window opened on ${host.name}. Finish the sign-in there.`}
        </p>
      )}
      {!signIn.isSuccess && mark?.label === 'Sign-in expired' && (
        <p className="settings-hint host-harness-status">
          {`${harness.name} refused a session on ${host.name}. The sign-in expired or was revoked. Sign in again.`}
        </p>
      )}
      {signIn.isError && (
        <p role="alert" className="settings-error host-harness-status">
          {errorMessage(signIn.error, 'The sign-in did not start.')}
        </p>
      )}
      {check.isError && (
        <p role="alert" className="settings-error host-harness-status">
          {errorMessage(check.error, 'The check did not start.')}
        </p>
      )}
    </Row>
  )
}

/** One row for each harness of the catalog, under a machine that runs
 *  commands. A machine with no `shell`, such as a phone, lists none. */
export function HostHarnesses({ api, host }: { api: ApiClient; host: HostDto }) {
  const catalog = useHarnesses(api)
  if (!host.capabilities.includes('shell')) return null
  return (
    <>
      {(catalog.data ?? []).map((harness) => (
        <HarnessRow key={harness.id} api={api} host={host} harness={harness} />
      ))}
    </>
  )
}
