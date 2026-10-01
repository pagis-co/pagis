import { type CookieJar, type CookieReader, SESSION_COOKIE, installationSession } from './clientSession'

/** How long the client waits for the server's answer. */
const TIMEOUT_MS = 5000

/**
 * The count of the Runs of the Local Installation that have not finished,
 * which a restart fails (ADR-0027), or null when the server does not give
 * it. `unfinished_runs` of the installation health holds it, and the
 * Administration Port answers it to an Administrator.
 *
 * The client signs in there with `installationSession`.
 */
export async function unfinishedRuns(
  administrationUrl: string,
  productUrl: string,
  credential: string | null,
  jar: CookieJar & CookieReader,
  request: typeof fetch = fetch,
): Promise<number | null> {
  try {
    const signal = AbortSignal.timeout(TIMEOUT_MS)
    const held = await installationSession(productUrl, credential, jar, request, signal)
    const response = await request(new URL('/api/v1/administration/health', administrationUrl), {
      redirect: 'error',
      signal,
      headers: { cookie: `${SESSION_COOKIE}=${held.secret}` },
    })
    if (!response.ok) throw new Error(`the installation health answered HTTP ${response.status}`)
    const count = ((await response.json()) as { unfinished_runs?: unknown }).unfinished_runs
    if (typeof count !== 'number' || !Number.isInteger(count) || count < 0) {
      throw new Error('the installation health holds no count of unfinished Runs')
    }
    return count
  } catch (error) {
    console.error(`pagis: the client did not count the unfinished Runs: ${String(error)}`)
    return null
  }
}
