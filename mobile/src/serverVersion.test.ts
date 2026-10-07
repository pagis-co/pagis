import semver from 'semver'
import { describe, expect, it } from 'vitest'

import { APP_VERSION, MINIMUM_SERVER_VERSION, serverVersionProblem } from './serverVersion'

describe('the server versions that the Mobile App accepts', () => {
  it('is its own release in the workspace, as package.json says', async () => {
    const manifest = (await import('../package.json')).default
    expect(APP_VERSION).toBe(manifest.version)
    expect(semver.valid(MINIMUM_SERVER_VERSION)).toBe(MINIMUM_SERVER_VERSION)
  })

  it('accepts the bound', () => {
    expect(serverVersionProblem(MINIMUM_SERVER_VERSION)).toBeNull()
  })

  /** The app loads the server's own Product App, and a store app and a
   *  self-hosted server update at different times. So the app refuses no
   *  newer server. */
  it('accepts every later version, also a later major version', () => {
    const bound = semver.parse(MINIMUM_SERVER_VERSION)!
    expect(serverVersionProblem(semver.inc(MINIMUM_SERVER_VERSION, 'patch')!)).toBeNull()
    expect(serverVersionProblem(semver.inc(MINIMUM_SERVER_VERSION, 'minor')!)).toBeNull()
    expect(serverVersionProblem(`${bound.major + 1}.0.0`)).toBeNull()
    expect(serverVersionProblem(`${bound.major + 7}.3.1`)).toBeNull()
  })

  it('tells the Person to ask the administrator to update a server below the bound', () => {
    const below = '0.0.1'

    expect(serverVersionProblem(below)).toBe(
      `This Pagis server runs ${below}, and this app is Pagis ${APP_VERSION}, which works with ` +
        `servers from ${MINIMUM_SERVER_VERSION} and later. Ask the administrator of the server to ` +
        `update it to ${MINIMUM_SERVER_VERSION} or newer.`,
    )
  })

  it('refuses a prerelease of the bound, which SemVer orders before it', () => {
    expect(serverVersionProblem(`${MINIMUM_SERVER_VERSION}-rc.1`)).toMatch(/Ask the administrator of the server/)
  })

  it('says that it does not understand a version that is not SemVer', () => {
    expect(serverVersionProblem('banana')).toBe(
      'This server reported Pagis version banana, which Pagis does not understand. Check the address.',
    )
    expect(serverVersionProblem('')).toBe(
      'This server reported Pagis version (none), which Pagis does not understand. Check the address.',
    )
  })
})
