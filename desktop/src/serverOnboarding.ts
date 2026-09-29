/**
 * Onboarding against a server the client did not start.
 *
 * It asks for one thing and nothing else: the server's address. There is
 * no key, no Docker, no port and no package, because the administrator
 * configured all of that once on the server and the client assumes the
 * installation is set up. Where it is not set up yet, the client says so
 * and names the page that finishes it. The Person signs in on the
 * server's own sign-in page, in the product window, as a chat client
 * such as Slack or Mattermost does: the client never holds the password.
 */

import { probeHealth } from './health'
import { serverOrigin } from './origin'
import { serverCompatibility } from './serverCompatibility'

/**
 * Check the server at the address the person typed, and answer its
 * origin.
 *
 * The order is the order the messages have to be right in: an address
 * that is not a server address is the person's to fix, a server that
 * does not answer is an address to fix, a server outside the
 * compatibility range is a version to fix, and a server with no
 * administrator is a setup to finish.
 */
export async function connectToServer(
  address: string,
  clientVersion: string,
  fetcher: typeof fetch = fetch,
  signal?: AbortSignal,
): Promise<string> {
  const origin = serverOrigin(address)
  await assertServerIsReady(origin, clientVersion, fetcher, signal)
  const setup = await openSetup(origin, fetcher, signal)
  if (setup !== null) {
    const page = setup.administrationOrigin === null
      ? 'the administration page, which answers on the Administration Port of the server itself'
      : `the administration page at ${setup.administrationOrigin}/ on the server itself`
    throw new Error(`This Pagis server has no administrator yet. Finish its setup on ${page}, then connect again.`)
  }
  return origin
}

/**
 * That a Pagis server answers at this origin and that this client can
 * work with its release.
 *
 * Every start of a connect-only client checks it, not only the first
 * one: the administrator updates the server when they choose, so the
 * release the client met yesterday is not the release it meets today.
 */
export async function assertServerIsReady(
  origin: string,
  clientVersion: string,
  fetcher: typeof fetch = fetch,
  signal?: AbortSignal,
): Promise<void> {
  const health = await probeHealth(origin, 5000, fetcher, signal)
  // A cancelled check says nothing of the server.
  signal?.throwIfAborted()
  if (!health) {
    throw new Error(
      `No Pagis server answered at ${origin}. Check the address and that the server is running.`,
    )
  }
  const incompatible = serverCompatibility(clientVersion, health.version)
  if (incompatible) throw new Error(incompatible)
}

/**
 * Whether the server's own first run is still open.
 *
 * `GET /api/v1/setup` answers while no administrator holds a password
 * and `410 Gone` from the first password onwards, so a `200` is the one
 * answer that means nobody can sign in yet. Any other answer is read as
 * a server that is set up: it is the sign-in that decides, and a
 * misreading here must not stand between a person and their workspace.
 *
 * When the request itself fails, the connection fails. A redirect makes
 * the request fail: TLS authenticates the server, and the client follows
 * no redirect away from it before the product window opens.
 */
async function openSetup(
  origin: string,
  fetcher: typeof fetch,
  signal?: AbortSignal,
): Promise<{ administrationOrigin: string | null } | null> {
  const response = await fetcher(new URL('/api/v1/setup', origin), {
    redirect: 'error',
    signal,
  })
  if (response.status !== 200) return null
  // The setup read names the Administration Port, which binds where the
  // server's deployment says, so the message names that and no default.
  const body = (await response.json().catch(() => ({}))) as { administration_origin?: unknown }
  return {
    administrationOrigin:
      typeof body.administration_origin === 'string' ? body.administration_origin : null,
  }
}
