import { describe, expect, it } from 'vitest'

import { checkForNewVersion, isNewer } from './updateCheck'

function releaseFetch(body: unknown, ok = true): typeof fetch {
  return (async () =>
    ({
      ok,
      json: async () => body,
    }) as Response) as unknown as typeof fetch
}

describe('the new-version check', () => {
  it('compares the release with the version the daemon reports', () => {
    expect(isNewer('0.2.0', '0.1.9')).toBe(true)
    expect(isNewer('0.1.10', '0.1.9')).toBe(true)
    expect(isNewer('0.1.0', '0.1.0')).toBe(false)
    expect(isNewer('0.0.9', '0.1.0')).toBe(false)
  })

  it('names the release when it is newer', async () => {
    const found = await checkForNewVersion(
      '0.1.0',
      releaseFetch({ tag_name: 'v0.2.0', html_url: 'https://example.test/0.2.0' }),
    )

    expect(found).toEqual({ version: '0.2.0', url: 'https://example.test/0.2.0' })
  })

  it('says nothing when the release is the version in use', async () => {
    const found = await checkForNewVersion(
      '0.2.0',
      releaseFetch({ tag_name: 'v0.2.0', html_url: 'https://example.test/0.2.0' }),
    )

    expect(found).toBeNull()
  })

  it('says nothing when the check fails', async () => {
    expect(await checkForNewVersion('0.1.0', releaseFetch({}, false))).toBeNull()
    const broken = (async () => {
      throw new Error('offline')
    }) as unknown as typeof fetch
    expect(await checkForNewVersion('0.1.0', broken)).toBeNull()
  })
})
