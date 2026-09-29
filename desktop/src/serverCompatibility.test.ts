import { describe, expect, it } from 'vitest'

import { compatibilityRange, serverCompatibility } from './serverCompatibility'

/**
 * The Runtime Lock keeps its exact-match rule where the client installs
 * the server. A connect-only client owns no version of the server, so it
 * checks a range instead (ADR-0025).
 */
describe('the compatibility range of a connect-only client', () => {
  it('is the SemVer range of its own release', () => {
    expect(compatibilityRange('1.4.2')).toBe('^1.4.2')
    expect(() => compatibilityRange('latest')).toThrow(/valid client version/)
  })

  it('accepts its own release and every later one of the same major', () => {
    expect(serverCompatibility('1.4.2', '1.4.2')).toBeNull()
    expect(serverCompatibility('1.4.2', '1.4.9')).toBeNull()
    expect(serverCompatibility('1.4.2', '1.7.0')).toBeNull()
  })

  it('tells the person to update the app when the server is ahead of the range', () => {
    expect(serverCompatibility('1.4.2', '2.0.0')).toBe(
      'This Pagis server runs 2.0.0, and this app is Pagis 1.4.2, which works with ' +
        'servers from 1.4.2 up to, but not including, 2.0.0. ' +
        'Update Pagis on this computer, then connect again.',
    )
  })

  it('tells them to ask the administrator when the server is behind', () => {
    expect(serverCompatibility('1.4.2', '1.4.1')).toBe(
      'This Pagis server runs 1.4.1, and this app is Pagis 1.4.2, which works with ' +
        'servers from 1.4.2 up to, but not including, 2.0.0. ' +
        'Ask the administrator of the server to update it to 1.4.2 or newer.',
    )
    expect(serverCompatibility('1.4.2', '0.9.0')).toContain('Ask the administrator')
  })

  /** A 0.x release promises nothing across a minor, and SemVer's own
   *  range says so, so a 0.2 client refuses a 0.3 server. */
  it('holds a pre-1.0 client to one minor', () => {
    expect(serverCompatibility('0.2.0', '0.2.7')).toBeNull()
    expect(serverCompatibility('0.2.0', '0.3.0')).toBe(
      'This Pagis server runs 0.3.0, and this app is Pagis 0.2.0, which works with ' +
        'servers from 0.2.0 up to, but not including, 0.3.0. ' +
        'Update Pagis on this computer, then connect again.',
    )
    expect(serverCompatibility('0.2.3', '0.1.9')).toBe(
      'This Pagis server runs 0.1.9, and this app is Pagis 0.2.3, which works with ' +
        'servers from 0.2.3 up to, but not including, 0.3.0. ' +
        'Ask the administrator of the server to update it to 0.2.3 or newer.',
    )
  })

  it('refuses an answer that is not a Pagis version at all', () => {
    expect(serverCompatibility('1.4.2', 'nightly')).toContain('Check the address')
    expect(serverCompatibility('1.4.2', '')).toContain('Check the address')
  })
})
