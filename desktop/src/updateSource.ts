/**
 * Where a check for an Update reads (ADR-0027).
 *
 * A Local Installation takes the latest release. A connected Client App
 * refuses a server older than itself (its Compatibility Range, ADR-0025),
 * so it takes only the Update to the release of its server, and never an
 * Update past it.
 */

import semver from 'semver'

import { probeHealth } from './health'
import { releaseUrl } from './runtimeLock'
import type { UpdateSource } from './updates'

/** How long a check waits for the health route of a server. */
const HEALTH_TIMEOUT_MS = 5000

/** The source of a Local Installation: the latest release of the GitHub
 *  releases of Pagis. Each check sets this feed again, because a connected
 *  client that becomes a Local Installation while it runs had the feed of
 *  its server. */
export const LATEST_RELEASE: UpdateSource = {
  kind: 'feed',
  feed: { provider: 'github', owner: 'pagis-co', repo: 'pagis' },
}

/**
 * The source of a connected Client App. It reads the release of the server
 * at `origin` from the health route at each check, because the
 * administrator updates the server when they choose.
 *
 * The feed of one release is the generic provider of electron-updater at
 * the download URL of that release. electron-updater reads the feed of the
 * platform there (`latest-mac.yml`, `latest-linux.yml` or
 * `latest-linux-arm64.yml`), and each file that the feed names, through
 * the redirect of GitHub. GitHub answers a request with several ranges
 * with 501, so the provider sends one range in each request, as the GitHub
 * provider does.
 */
export async function serverSource(
  origin: string,
  clientVersion: string,
  request: typeof fetch = fetch,
): Promise<UpdateSource> {
  const health = await probeHealth(origin, HEALTH_TIMEOUT_MS, request)
  if (!health) {
    throw new Error(`No Pagis server answered at ${origin}, so Pagis cannot read the release of its server.`)
  }
  const server = health.version
  // The release becomes a part of the URL of the feed.
  if (semver.valid(server) !== server) {
    throw new Error(`The Pagis server at ${origin} reported the release "${server}", which is not a release version.`)
  }
  if (!semver.gt(server, clientVersion)) return { kind: 'server-not-newer', server }
  return {
    kind: 'feed',
    feed: { provider: 'generic', url: releaseUrl(server), useMultipleRangeRequest: false },
  }
}
