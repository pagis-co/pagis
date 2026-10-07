/**
 * The checks of the Connect screen, before the app opens a server.
 *
 * They are the checks of the Client App (`connectToServer` in
 * `desktop/src/serverOnboarding.ts`), in the same order and with the same
 * words. The order is the order the messages have to be right in: an
 * address that is not a server address is the Person's to fix, a server
 * that does not answer is an address to fix, a server below the bound is
 * a version to fix, and a server with no administrator is a setup to
 * finish. Every check goes to the origin, and no request holds the
 * secret of a link.
 *
 * A page that the app bundles cannot read the health route with `fetch`,
 * because the CORS answer of the server names its Public Origin alone. So
 * each request goes through the native HTTP client of Capacitor,
 * `CapacitorHttp.request`, which the caller gives.
 */

import type { HttpOptions, HttpResponse } from '@capacitor/core'

import { type BuildType, type ServerAddress, scannedSignInLink, serverAddress } from './address'
import { serverVersionProblem } from './serverVersion'

/** One HTTP request through the native client: `CapacitorHttp.request`. */
export type HttpRequest = (options: HttpOptions) => Promise<HttpResponse>

/** How long a check waits for the server, in milliseconds. */
const TIMEOUT_MS = 5000

/** Where to get a Sign-In Link. The words of the sign-in page of the
 *  Product App (`WHERE_TO_GET_A_LINK` in `ui/src/queries.ts`). */
const WHERE_TO_GET_A_LINK =
  'Make a new link in Settings → Sessions on a browser or app that is signed in. Or ask ' +
  'an Administrator for a new invite, or run "pagis pair" on the machine of the server.'

/** What a check of a server needs. */
export type ConnectOptions = BuildType & { request: HttpRequest }

/**
 * Check the server at the address or the Sign-In Link that the Person
 * typed. Answer the origin to keep and the address that the web view
 * opens.
 */
export async function connectToServer(typed: string, options: ConnectOptions): Promise<ServerAddress> {
  return checkServer(serverAddress(typed, options), options)
}

/**
 * Check the server of a scanned Sign-In Link, with the checks of a typed
 * one. A scan that holds no Sign-In Link is refused before any request.
 */
export async function connectWithScannedLink(scanned: string, options: ConnectOptions): Promise<ServerAddress> {
  return checkServer(scannedSignInLink(scanned, options), options)
}

async function checkServer(address: ServerAddress, options: ConnectOptions): Promise<ServerAddress> {
  const { origin } = address
  const health = await readHealth(origin, options.request)
  const problem = serverVersionProblem(health.version)
  if (problem !== null) throw new Error(problem)
  const setup = await openSetup(origin, options.request)
  if (setup !== null) {
    const page = setup.administrationOrigin === null
      ? 'the administration page, which answers on the Administration Port of the server itself'
      : `the administration page at ${setup.administrationOrigin}/ on the server itself`
    throw new Error(`This Pagis server has no administrator yet. Finish its setup on ${page}, then connect again.`)
  }
  // In Remote Access the server takes no password from another machine
  // (ADR-0028), so its sign-in page would ask this app for a link.
  if (health.signIn === 'link' && address.opens === origin) {
    throw new Error(
      `This Pagis server signs in this app with a sign-in link only. Paste a sign-in link of the server. ${WHERE_TO_GET_A_LINK}`,
    )
  }
  return address
}

/** What `GET /api/v1/health` answers. The route needs no Session. */
interface Health {
  version: string
  /** How this app signs in: `link` from another machine in Remote
   *  Access, else `password`. */
  signIn: string
}

async function readHealth(origin: string, request: HttpRequest): Promise<Health> {
  const response = await get(origin, '/api/v1/health', request)
  const body = response.status >= 200 && response.status < 300 ? asObject(response.data) : null
  if (body === null || body.status !== 'ok' || typeof body.version !== 'string') {
    throw new Error(noServerAnswered(origin))
  }
  return { version: body.version, signIn: typeof body.sign_in === 'string' ? body.sign_in : 'password' }
}

/**
 * Whether the server's own first run is still open.
 *
 * `GET /api/v1/setup` answers while no administrator holds a password
 * and `410 Gone` from the first password onwards, so a `200` is the one
 * answer that means nobody can sign in yet. Any other answer is read as
 * a server that is set up: it is the sign-in that decides.
 */
async function openSetup(
  origin: string,
  request: HttpRequest,
): Promise<{ administrationOrigin: string | null } | null> {
  const response = await get(origin, '/api/v1/setup', request)
  if (response.status !== 200) return null
  // The setup read names the Administration Port, which binds where the
  // deployment of the server says, so the message names that and no
  // default.
  const body = asObject(response.data) ?? {}
  return {
    administrationOrigin: typeof body.administration_origin === 'string' ? body.administration_origin : null,
  }
}

/**
 * Send one GET to a route of the origin, or say that no server answered.
 *
 * It follows no redirect: TLS authenticates the server, and a redirect
 * could take the app to another server, or to `http://`, before it opens
 * the server. With `disableRedirects` the native client gives the
 * redirect back as the answer, and the check stops there.
 */
async function get(origin: string, path: string, request: HttpRequest): Promise<HttpResponse> {
  let response: HttpResponse
  try {
    response = await request({
      url: new URL(path, origin).href,
      method: 'GET',
      disableRedirects: true,
      connectTimeout: TIMEOUT_MS,
      readTimeout: TIMEOUT_MS,
      responseType: 'json',
    })
  } catch {
    throw new Error(noServerAnswered(origin))
  }
  if (response.status >= 300 && response.status < 400) {
    throw new Error(
      `The server at ${origin} sent this app to another address. Pagis follows no redirect before it ` +
        'opens a server. Type the address that the server answers on.',
    )
  }
  return response
}

function noServerAnswered(origin: string): string {
  return `No Pagis server answered at ${origin}. Check the address and that the server is running.`
}

/** The JSON object of an answer. The native client parses a JSON answer,
 *  and gives back text where the server named another content type. */
function asObject(data: unknown): Record<string, unknown> | null {
  let value = data
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value)
    } catch {
      return null
    }
  }
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}
