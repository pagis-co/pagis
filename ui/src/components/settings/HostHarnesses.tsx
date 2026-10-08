// The Coding Harnesses of one machine, under its row in Settings › Hosts
// (ADR-0033): whether the machine can start each harness of the Harness
// Catalog, whether the daemon reports that it needs a sign-in there, and
// a Harness Sign-In for each of its methods.
//
// The Product App only starts the sign-in. The Person finishes it in the
// vendor's own program in a terminal window on that machine, so no field
// here takes a credential (ADR-0022).

import type { ApiClient, HarnessDto, HarnessSignInMethod, HostDto } from '../../api/client'
import { Badge, Button, Row } from '../../primitives'
import { errorMessage, useHarnesses, useStartHarnessSignIn } from '../../queries'

import '../settings.css'
import './HostHarnesses.css'

/** The words of the button of each sign-in method. */
const SIGN_IN_BUTTONS: Record<HarnessSignInMethod, string> = {
  subscription: 'Sign in with a subscription',
  api_key: 'Sign in with an API key',
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
  // The Host gives one entry for each `harness:<id>` capability, with
  // the report of the daemon.
  const declared = host.harnesses.find((entry) => entry.id === harness.id)
  const canStart = declared !== undefined

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
      {declared?.needs_sign_in === true && <Badge tone="waiting">Needs sign-in</Badge>}
      <div className="settings-row-actions host-harness-actions">
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
          {`A terminal window opened on ${host.name}. Finish the sign-in there.`}
        </p>
      )}
      {signIn.isError && (
        <p role="alert" className="settings-error host-harness-status">
          {errorMessage(signIn.error, 'The sign-in did not start.')}
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
