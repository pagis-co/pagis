import semver from 'semver'

import { type CookieJar, type CookieReader, SESSION_COOKIE, installationSession } from './clientSession'
import {
  type ClientPlatform,
  type RuntimeLock,
  parseRuntimeLock,
  releaseAssetUrl,
  runtimeLockFile,
  thisPlatform,
} from './runtimeLock'

/** How long the client waits for the Runtime Lock of a release. */
const LOCK_TIMEOUT_MS = 30 * 1000
/** How long the client waits for the daemon to pull a Computer Image. A
 *  pull on a slow line takes many minutes. When the client stops waiting,
 *  the daemon continues the pull, and the Update becomes ready. */
const PULL_TIMEOUT_MS = 30 * 60 * 1000

/** The steps of a preparation. */
export interface PreparationSteps {
  /** Read the Runtime Lock that the release `version` publishes. */
  readLock(version: string): Promise<RuntimeLock>
  /** Put the Server Package that the lock names into the download cache. */
  download(lock: RuntimeLock): Promise<void>
  /** Ask the daemon to pull `image`, and wait for its answer. */
  pull(image: string): Promise<void>
}

/** Where the client reaches the daemon of its Local Installation. */
export interface Installation {
  administrationUrl: string
  productUrl: string
  credential: string | null
  jar: CookieJar & CookieReader
}

/**
 * Prepare the restart of a Local Installation to the Update `version`
 * (ADR-0027). The client reads the Runtime Lock of that release, puts its
 * Server Package into the download cache, and asks the daemon to pull its
 * Computer Image. The download and the pull run at the same time.
 *
 * Each failed step is logged and stops only the steps that need it: the
 * Update continues, and the new Client App and the new daemon download
 * what is missing. The preparation itself does not fail.
 */
export async function prepareUpdate(
  version: string,
  steps: PreparationSteps,
  log: (line: string) => void = (line) => console.error(line),
): Promise<void> {
  let lock: RuntimeLock
  try {
    lock = await steps.readLock(version)
  } catch (error) {
    log(`pagis: the client did not read the Runtime Lock of Pagis ${version}: ${reasonOf(error)}`)
    return
  }
  await Promise.all([
    steps.download(lock).catch((error: unknown) => {
      log(`pagis: the client did not download the Server Package of Pagis ${version}: ${reasonOf(error)}`)
    }),
    steps.pull(lock.computer_image).catch((error: unknown) => {
      log(`pagis: the daemon did not pull the Computer Image of Pagis ${version}: ${reasonOf(error)}`)
    }),
  ])
}

/**
 * The Runtime Lock of this platform that the release `version` publishes.
 * It only names what to fetch, and it is not a trust root (ADR-0025): the
 * new Client App checks the cached bytes against its own embedded lock.
 */
export async function releaseLock(
  version: string,
  request: typeof fetch = fetch,
  host: ClientPlatform = thisPlatform,
): Promise<RuntimeLock> {
  // The version comes from the feed, which is not a trust root either, and
  // it becomes a part of the URL.
  if (semver.valid(version) !== version) throw new Error(`"${version}" is not a release version`)
  const url = releaseAssetUrl(version, runtimeLockFile(host))
  const response = await request(url, {
    signal: AbortSignal.timeout(LOCK_TIMEOUT_MS),
    headers: { 'user-agent': 'Pagis' },
  })
  if (!response.ok) throw new Error(`${url} answered HTTP ${response.status}`)
  return parseRuntimeLock(await response.text(), version, host)
}

/**
 * Ask the daemon of the Local Installation to pull `image`, and wait for
 * the end of the pull. The Administration Port answers an Administrator,
 * and the client signs in with `installationSession`. The daemon pulls in
 * a task of its own, so a request that ends early does not stop the pull.
 */
export async function pullComputerImage(
  image: string,
  installation: Installation,
  request: typeof fetch = fetch,
): Promise<void> {
  const signal = AbortSignal.timeout(PULL_TIMEOUT_MS)
  const { administrationUrl, productUrl, credential, jar } = installation
  const held = await installationSession(productUrl, credential, jar, request, signal)
  const response = await request(new URL('/api/v1/administration/computer-image/pull', administrationUrl), {
    method: 'POST',
    redirect: 'error',
    signal,
    headers: { 'content-type': 'application/json', cookie: `${SESSION_COOKIE}=${held.secret}` },
    body: JSON.stringify({ image }),
  })
  if (!response.ok) {
    throw new Error(`the daemon answered HTTP ${response.status}: ${await errorMessage(response)}`)
  }
}

/** The message of an error answer of the daemon, `{error: {message}}`. */
async function errorMessage(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: { message?: unknown } }
    if (typeof body.error?.message === 'string') return body.error.message
  } catch {
    // The body is not the error shape of the daemon.
  }
  return 'no reason'
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
