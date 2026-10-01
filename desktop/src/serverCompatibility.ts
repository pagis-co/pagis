/**
 * What a connect-only client accepts of a server it did not start
 * (ADR-0025).
 *
 * The Runtime Lock is the trust root for a download: where the client
 * installs the server, the release, the platform, the package contents
 * and the Computer image are one exact tuple and nothing else runs. That
 * rule cannot hold here, because the administrator owns the server's
 * version and updates it when they choose. A connect-only client
 * downloads no server, so it needs a different rule for a different
 * problem: a compatibility range over the version the server reports.
 *
 * The range is SemVer's own: the client accepts its own release and
 * every later release that promises the same API. A server below the
 * client may not hold a route this client calls; a server above the
 * range may have dropped one.
 */

import semver from 'semver'

/** The range this client accepts, as SemVer writes it. */
export function compatibilityRange(clientVersion: string): string {
  if (semver.valid(clientVersion) === null) {
    throw new Error(`${clientVersion} is not a valid client version`)
  }
  return `^${clientVersion}`
}

/**
 * The range in words a person reads: "servers from 0.1.0 up to, but not
 * including, 0.2.0".
 *
 * The bounds come from SemVer's own parse of the range. The upper bound
 * carries the prerelease tag `-0`, which only keeps prereleases out of
 * the range, so the words drop it.
 */
function rangeInWords(range: string): string {
  const [lower, upper] = new semver.Range(range).set[0]
  const { major, minor, patch } = upper.semver
  return `servers from ${lower.semver.version} up to, but not including, ${major}.${minor}.${patch}`
}

/**
 * Why this client cannot work with that server, or null when it can.
 *
 * The message says what to do about it, and which end is behind decides
 * which thing that is: the person updates their own app, or the
 * administrator updates the server nobody else can.
 */
export function serverCompatibility(
  clientVersion: string,
  serverVersion: string,
): string | null {
  const range = compatibilityRange(clientVersion)
  if (semver.valid(serverVersion) === null) {
    return `This server reported Pagis version ${serverVersion || '(none)'}, which Pagis does not understand. Check the address.`
  }
  if (semver.satisfies(serverVersion, range)) return null
  if (semver.gt(serverVersion, clientVersion)) {
    return (
      `This Pagis server runs ${serverVersion}, and this app is Pagis ${clientVersion}, ` +
      `which works with ${rangeInWords(range)}. Update Pagis on this computer, then connect again.`
    )
  }
  return (
    `This Pagis server runs ${serverVersion}, and this app is Pagis ${clientVersion}, ` +
    `which works with ${rangeInWords(range)}. Ask the administrator of the server to update it to ` +
    `${clientVersion} or newer.`
  )
}
