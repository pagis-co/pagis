import { describe, expect, it } from 'vitest'

import { NoRuntimeLockError, parseRuntimeLock, readRuntimeLock, runtimeLockFile } from './runtimeLock'

const SHA = 'a'.repeat(64)
const MAC = { platform: 'darwin', arch: 'arm64' }
const LINUX = { platform: 'linux', arch: 'x64' }

function macLock(release = '0.1.0'): Record<string, unknown> {
  return {
    schema: 1,
    release,
    platform: 'darwin',
    arch: 'arm64',
    asset: {
      format: 'dmg',
      name: `pagis-server-${release}-aarch64-apple-darwin.dmg`,
      url: `https://github.com/pagis-co/pagis/releases/download/v${release}/pagis-server-${release}-aarch64-apple-darwin.dmg`,
      size: 123,
      sha256: SHA,
      team_id: 'ABCDE12345',
    },
    entries: [
      { path: 'pagis', kind: 'executable', codesign_id: 'com.pagis.server', size: 10, sha256: SHA, mode: 0o755 },
      { path: 'gog', kind: 'executable', codesign_id: 'com.pagis.gog', size: 11, sha256: SHA, mode: 0o755 },
      { path: 'LICENSE', kind: 'file', size: 12, sha256: SHA, mode: 0o644 },
      { path: 'LICENSE.gog', kind: 'file', size: 12, sha256: SHA, mode: 0o644 },
      { path: 'THIRD_PARTY_NOTICES', kind: 'file', size: 13, sha256: SHA, mode: 0o644 },
    ],
    computer_image: `ghcr.io/pagis-co/pagis-computer@sha256:${SHA}`,
  }
}

function linuxLock(arch = 'x64', release = '0.1.0'): Record<string, unknown> {
  const triple = arch === 'x64' ? 'x86_64-unknown-linux-gnu' : 'aarch64-unknown-linux-gnu'
  const name = `pagis-server-${release}-${triple}.tar.gz`
  return {
    schema: 1,
    release,
    platform: 'linux',
    arch,
    asset: {
      format: 'tar.gz',
      name,
      url: `https://github.com/pagis-co/pagis/releases/download/v${release}/${name}`,
      size: 123,
      sha256: SHA,
    },
    entries: [
      { path: 'pagis', kind: 'executable', size: 10, sha256: SHA, mode: 0o755 },
      { path: 'gog', kind: 'executable', size: 11, sha256: SHA, mode: 0o755 },
      { path: 'LICENSE', kind: 'file', size: 12, sha256: SHA, mode: 0o644 },
      { path: 'LICENSE.gog', kind: 'file', size: 12, sha256: SHA, mode: 0o644 },
      { path: 'THIRD_PARTY_NOTICES', kind: 'file', size: 13, sha256: SHA, mode: 0o644 },
    ],
    computer_image: `ghcr.io/pagis-co/pagis-computer@sha256:${SHA}`,
  }
}

describe('the client Runtime Lock', () => {
  it('accepts only the client release and supported platform tuple', () => {
    expect(parseRuntimeLock(JSON.stringify(macLock()), '0.1.0', MAC)).toMatchObject({
      release: '0.1.0',
      platform: 'darwin',
      arch: 'arm64',
    })

    expect(() => parseRuntimeLock(JSON.stringify(macLock()), '0.2.0', MAC)).toThrow(/release/)
    expect(() => parseRuntimeLock(JSON.stringify({ ...macLock(), arch: 'x64' }), '0.1.0', MAC)).toThrow(/platform|client/)
  })

  it('accepts a lock only on the platform and architecture it names', () => {
    expect(() => parseRuntimeLock(JSON.stringify(macLock()), '0.1.0', LINUX)).toThrow(/linux x64 client/)
    expect(() => parseRuntimeLock(JSON.stringify(linuxLock('arm64')), '0.1.0', LINUX)).toThrow(/linux x64 client/)
    expect(() => parseRuntimeLock(JSON.stringify(linuxLock('x64')), '0.1.0', MAC)).toThrow(/darwin arm64 client/)
    expect(() => parseRuntimeLock(JSON.stringify({ ...macLock(), arch: 'x64' }), '0.1.0', { platform: 'darwin', arch: 'x64' }))
      .toThrow(/unsupported platform/)
  })

  it('rejects missing, extra and malformed trust fields', () => {
    const extra = macLock()
    extra.latest = true
    expect(() => parseRuntimeLock(JSON.stringify(extra), '0.1.0', MAC)).toThrow(/field/)

    const missing = macLock()
    delete missing.computer_image
    expect(() => parseRuntimeLock(JSON.stringify(missing), '0.1.0', MAC)).toThrow(/field/)

    const malformed = macLock() as { asset: { sha256: string } }
    malformed.asset.sha256 = 'ABC'
    expect(() => parseRuntimeLock(JSON.stringify(malformed), '0.1.0', MAC)).toThrow(/sha256/)
  })

  it('rejects any package layout other than the five locked root files', () => {
    const changed = macLock() as { entries: Array<Record<string, unknown>> }
    changed.entries[0].path = '../pagis'
    expect(() => parseRuntimeLock(JSON.stringify(changed), '0.1.0', MAC)).toThrow(/entries/)

    const duplicate = macLock() as { entries: Array<Record<string, unknown>> }
    duplicate.entries[1].path = 'pagis'
    expect(() => parseRuntimeLock(JSON.stringify(duplicate), '0.1.0', MAC)).toThrow(/entries/)
  })

  it('accepts SemVer prerelease and build metadata and rejects invalid releases', () => {
    expect(parseRuntimeLock(JSON.stringify(macLock('1.0.0-rc.10+signed.7')), '1.0.0-rc.10+signed.7', MAC).release)
      .toBe('1.0.0-rc.10+signed.7')
    expect(() => parseRuntimeLock(JSON.stringify(macLock('1.0.0-01')), '1.0.0-01', MAC))
      .toThrow(/SemVer/)
  })
})

describe('the Linux Runtime Lock', () => {
  it('names the server archive of its architecture', () => {
    for (const arch of ['x64', 'arm64']) {
      const lock = parseRuntimeLock(JSON.stringify(linuxLock(arch)), '0.1.0', { platform: 'linux', arch })
      expect(lock).toMatchObject({ platform: 'linux', arch, asset: { format: 'tar.gz' } })
    }

    const wrongArchive = linuxLock('x64') as { asset: { name: string } }
    wrongArchive.asset.name = 'pagis-server-0.1.0-aarch64-unknown-linux-gnu.tar.gz'
    expect(() => parseRuntimeLock(JSON.stringify(wrongArchive), '0.1.0', LINUX)).toThrow(/exact release tar.gz/)
  })

  it('carries no macOS signing field', () => {
    const team = linuxLock() as { asset: Record<string, unknown> }
    team.asset.team_id = 'ABCDE12345'
    expect(() => parseRuntimeLock(JSON.stringify(team), '0.1.0', LINUX)).toThrow(/field/)

    const identity = linuxLock() as { entries: Array<Record<string, unknown>> }
    identity.entries[0].codesign_id = 'com.pagis.server'
    expect(() => parseRuntimeLock(JSON.stringify(identity), '0.1.0', LINUX)).toThrow(/field/)
  })

  it('requires the macOS signing fields on macOS', () => {
    const unsigned = macLock() as { entries: Array<Record<string, unknown>> }
    delete unsigned.entries[0].codesign_id
    expect(() => parseRuntimeLock(JSON.stringify(unsigned), '0.1.0', MAC)).toThrow(/field/)
  })

  it('has one file name for each platform', () => {
    expect(runtimeLockFile(MAC)).toBe('runtime-lock-darwin-arm64.json')
    expect(runtimeLockFile({ platform: 'linux', arch: 'arm64' })).toBe('runtime-lock-linux-arm64.json')
  })
})

describe('reading the Runtime Lock', () => {
  const FILE = '/Users/ada/pagis/dist/runtime-lock-darwin-arm64.json'

  it('reads the lock of this client', () => {
    expect(readRuntimeLock(FILE, '0.1.0', () => JSON.stringify(macLock()), MAC).release).toBe('0.1.0')
  })

  // A build from source with no lock, or with a lock it cannot use, has
  // no Server Runtime to install. The message says that, and names no
  // file on the developer's disk.
  it('says a build with no usable lock has no Server Runtime to install', () => {
    const missing = () => {
      throw Object.assign(new Error(`ENOENT: no such file or directory, open '${FILE}'`), { code: 'ENOENT' })
    }
    for (const read of [missing, () => 'not json', () => JSON.stringify(macLock('0.2.0'))]) {
      let thrown: unknown
      try {
        readRuntimeLock(FILE, '0.1.0', read, MAC)
      } catch (error) {
        thrown = error
      }
      expect(thrown).toBeInstanceOf(NoRuntimeLockError)
      const message = (thrown as Error).message
      expect(message).toMatch(/no Server Runtime to install/)
      expect(message).not.toContain('/Users/ada')
      expect(message).not.toContain('ENOENT')
    }
  })
})
