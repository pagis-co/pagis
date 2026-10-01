/** The release the check found, when it is newer than the daemon's. */
export interface NewVersion {
  version: string
  url: string
}

const LATEST_RELEASE = 'https://api.github.com/repos/pagis-co/pagis/releases/latest'

/**
 * The new-version line of the tray on Linux, where the Client App has no
 * updater: it reads the latest release and says that one is there, with a
 * link.
 */
export async function checkForNewVersion(
  current: string,
  fetchImpl: typeof fetch = fetch,
): Promise<NewVersion | null> {
  try {
    const response = await fetchImpl(LATEST_RELEASE, {
      headers: { accept: 'application/vnd.github+json' },
      signal: AbortSignal.timeout(5000),
    })
    if (!response.ok) {
      return null
    }
    const release = (await response.json()) as { tag_name?: string; html_url?: string }
    if (!release.tag_name || !release.html_url) {
      return null
    }
    const version = release.tag_name.replace(/^v/, '')
    return isNewer(version, current) ? { version, url: release.html_url } : null
  } catch {
    return null
  }
}

/** Compare two `major.minor.patch` versions. */
export function isNewer(candidate: string, current: string): boolean {
  const left = numbers(candidate)
  const right = numbers(current)
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    const difference = (left[index] ?? 0) - (right[index] ?? 0)
    if (difference !== 0) {
      return difference > 0
    }
  }
  return false
}

function numbers(version: string): number[] {
  return version
    .split('-')[0]
    .split('.')
    .map((part) => Number(part))
    .map((part) => (Number.isInteger(part) ? part : 0))
}
