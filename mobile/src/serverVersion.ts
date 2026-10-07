/**
 * The server versions that the Mobile App accepts: a lower bound, and no
 * upper bound (ADR-0025, ADR-0032).
 *
 * The web part of the app is the Product App of the server, so it always
 * matches the server. The native part reads only the payload format of a
 * Notification and the decision route, which are a stable contract
 * (ADR-0030). A store app and a self-hosted server update at different
 * times, so the app refuses no newer server.
 */

import semver from 'semver'

import { version } from '../package.json'

/** The release of this app. */
export const APP_VERSION: string = version

/** The oldest server release that this app accepts. */
export const MINIMUM_SERVER_VERSION = '0.2.0'

/**
 * Why this app cannot work with a server of this version, or null when
 * it can. The words are the words of the Client App
 * (`desktop/src/serverCompatibility.ts`).
 */
export function serverVersionProblem(serverVersion: string): string | null {
  if (semver.valid(serverVersion) === null) {
    return `This server reported Pagis version ${serverVersion || '(none)'}, which Pagis does not understand. Check the address.`
  }
  if (semver.gte(serverVersion, MINIMUM_SERVER_VERSION)) return null
  return (
    `This Pagis server runs ${serverVersion}, and this app is Pagis ${APP_VERSION}, which works with ` +
    `servers from ${MINIMUM_SERVER_VERSION} and later. Ask the administrator of the server to update it ` +
    `to ${MINIMUM_SERVER_VERSION} or newer.`
  )
}
