import { execFileSync } from 'node:child_process'
import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as http from 'node:http'
import * as net from 'node:net'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import {
  RuntimeInstaller,
  downloadLockedAsset,
  extractArchive,
  installationError,
  type DarwinAdapters,
  type HttpsGet,
  type LinuxAdapters,
} from './runtimeInstaller'
import type { DarwinEntry, DarwinRuntimeLock, LinuxRuntimeLock } from './runtimeLock'

const roots: string[] = []
afterEach(() => {
  while (roots.length > 0) fs.rmSync(roots.pop()!, { recursive: true, force: true })
})

function fixture(extra?: string): { root: string; lock: DarwinRuntimeLock; source: string } {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-installer-'))
  roots.push(root)
  const source = path.join(root, 'mounted')
  fs.mkdirSync(source)
  const specs = [
    ['pagis', 'server', 0o755, 'executable', 'com.pagis.server'],
    ['gog', 'gog', 0o755, 'executable', 'com.pagis.gog'],
    ['LICENSE', 'pagis license', 0o644, 'file', undefined],
    ['LICENSE.gog', 'license', 0o644, 'file', undefined],
    ['THIRD_PARTY_NOTICES', 'notices', 0o644, 'file', undefined],
  ] as const
  const entries = specs.map(([name, body, mode, kind, codesign_id]) => {
    const file = path.join(source, name)
    fs.writeFileSync(file, body, { mode })
    return {
      path: name,
      kind,
      ...(codesign_id ? { codesign_id } : {}),
      size: Buffer.byteLength(body),
      sha256: crypto.createHash('sha256').update(body).digest('hex'),
      mode,
    }
  })
  if (extra) fs.writeFileSync(path.join(source, extra), 'bad')
  const dmg = Buffer.from('fixture dmg')
  const release = '0.1.0'
  return {
    root,
    source,
    lock: {
      schema: 1,
      release,
      platform: 'darwin',
      arch: 'arm64',
      asset: {
        format: 'dmg',
        name: `pagis-server-${release}-aarch64-apple-darwin.dmg`,
        url: `https://github.com/pagis-co/pagis/releases/download/v${release}/pagis-server-${release}-aarch64-apple-darwin.dmg`,
        size: dmg.length,
        sha256: crypto.createHash('sha256').update(dmg).digest('hex'),
        team_id: 'ABCDE12345',
      },
      entries: entries as DarwinEntry[],
      computer_image: `ghcr.io/pagis-co/pagis-computer@sha256:${'a'.repeat(64)}`,
    },
  }
}

function adapters(source: string, calls: string[]): DarwinAdapters {
  return {
    platform: 'darwin',
    recoverMount: async () => {},
    activate: async (staged, destination) => { fs.renameSync(staged, destination) },
    download: async (_url, destination) => {
      fs.writeFileSync(destination, 'fixture dmg')
      calls.push('download')
    },
    mountReadOnly: async (_dmg, mountpoint) => {
      calls.push('mount-read-only')
      fs.cpSync(source, mountpoint, { recursive: true })
      for (const name of fs.readdirSync(source)) {
        const metadata = fs.lstatSync(path.join(source, name))
        if (metadata.isFile()) fs.chmodSync(path.join(mountpoint, name), metadata.mode & 0o7777)
      }
      return async () => { calls.push('detach') }
    },
    verifyImage: async () => { calls.push('verify-image') },
    quarantine: async (file, value) => {
      calls.push(`${value === undefined ? 'read' : 'write'}-quarantine:${path.basename(file)}`)
      return value ?? '0083;fixture;Pagis;https://github.com/'
    },
    verifyExecutable: async (file, identifier, team) => {
      calls.push(`verify:${path.basename(file)}:${identifier}:${team}`)
    },
  }
}

describe('the client Runtime installer', () => {
  it('downloads, verifies and atomically installs only the locked tree', async () => {
    const { root, lock, source } = fixture()
    const calls: string[] = []
    const installer = new RuntimeInstaller(path.join(root, 'runtime'), adapters(source, calls))

    const installed = await installer.install(lock)

    expect(installed).toBe(path.join(root, 'runtime', 'releases', '0.1.0', 'darwin-arm64', 'pagis'))
    expect(fs.readFileSync(installed, 'utf8')).toBe('server')
    expect(calls).toEqual([
      'download',
      `read-quarantine:${lock.asset.sha256}.part`,
      'verify-image',
      'mount-read-only',
      'write-quarantine:pagis',
      'verify:pagis:com.pagis.server:ABCDE12345',
      'write-quarantine:gog',
      'verify:gog:com.pagis.gog:ABCDE12345',
      'detach',
    ])
  })

  it('rejects extra mounted entries before it copies or executes a file', async () => {
    const { root, lock, source } = fixture('surprise')
    const calls: string[] = []
    const installer = new RuntimeInstaller(path.join(root, 'runtime'), adapters(source, calls))

    await expect(installer.install(lock)).rejects.toThrow(/unexpected/)
    expect(fs.existsSync(path.join(root, 'runtime', 'releases', '0.1.0'))).toBe(false)
    expect(calls.some((call) => call.startsWith('verify:'))).toBe(false)
  })

  it('keeps the installed release unchanged when a later check fails', async () => {
    const { root, lock, source } = fixture()
    const destination = path.join(root, 'runtime')
    const prior = path.join(destination, 'releases', '0.0.9', 'darwin-arm64', 'pagis')
    fs.mkdirSync(path.dirname(prior), { recursive: true })
    fs.writeFileSync(prior, 'previous')
    fs.writeFileSync(path.join(source, 'pagis'), 'changed', { mode: 0o755 })
    const installer = new RuntimeInstaller(destination, adapters(source, []))

    await expect(installer.install(lock)).rejects.toThrow(/pagis.*size|pagis.*hash/)
    expect(fs.readFileSync(prior, 'utf8')).toBe('previous')
  })

  it('rejects wrong bytes, modes and links before activation', async () => {
    for (const mutate of [
      (source: string) => fs.writeFileSync(path.join(source, 'pagis'), 'changed', { mode: 0o755 }),
      (source: string) => fs.chmodSync(path.join(source, 'pagis'), 0o644),
      (source: string) => {
        const binary = path.join(source, 'pagis')
        fs.chmodSync(binary, 0o4755)
        expect(fs.lstatSync(binary).mode & 0o7777).toBe(0o4755)
      },
      (source: string) => {
        fs.rmSync(path.join(source, 'pagis'))
        fs.symlinkSync(path.join(source, 'gog'), path.join(source, 'pagis'))
      },
    ]) {
      const { root, lock, source } = fixture()
      mutate(source)
      await expect(new RuntimeInstaller(path.join(root, 'runtime'), adapters(source, [])).install(lock)).rejects.toThrow()
      expect(fs.existsSync(path.join(root, 'runtime', 'releases', lock.release))).toBe(false)
    }
  })

  it('rejects absent quarantine and failed code identity checks', async () => {
    const absent = fixture()
    const noQuarantine = adapters(absent.source, [])
    noQuarantine.quarantine = async () => ''
    await expect(new RuntimeInstaller(path.join(absent.root, 'runtime'), noQuarantine).install(absent.lock)).rejects.toThrow(/quarantine/)

    const unsigned = fixture()
    const wrongIdentity = adapters(unsigned.source, [])
    wrongIdentity.verifyExecutable = async () => { throw new Error('wrong signing identity') }
    await expect(new RuntimeInstaller(path.join(unsigned.root, 'runtime'), wrongIdentity).install(unsigned.lock)).rejects.toThrow(/signing identity/)
    expect(fs.existsSync(path.join(unsigned.root, 'runtime', 'releases', unsigned.lock.release))).toBe(false)
  })

  it('rejects changed outer asset bytes before image verification', async () => {
    const { root, lock, source } = fixture()
    const calls: string[] = []
    const changed = adapters(source, calls)
    changed.download = async (_url, destination) => { fs.writeFileSync(destination, 'changed') }

    await expect(new RuntimeInstaller(path.join(root, 'runtime'), changed).install(lock)).rejects.toThrow(/wrong size|wrong hash/)
    expect(calls).toEqual([])
  })

  it('leaves a damaged installed runtime in place for a running server', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const installer = new RuntimeInstaller(runtime, adapters(source, []))
    const installed = await installer.install(lock)
    fs.writeFileSync(installed, 'tampered', { mode: 0o755 })

    await expect(installer.install(lock)).rejects.toThrow(/wrong size|wrong hash/)
    expect(fs.readFileSync(installed, 'utf8')).toBe('tampered')
  })

  it('replaces a damaged runtime only after its replacement is verified and ownership is released', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const calls: string[] = []
    const installer = new RuntimeInstaller(runtime, adapters(source, calls))
    const installed = await installer.install(lock)
    fs.writeFileSync(installed, 'tampered', { mode: 0o755 })
    calls.length = 0

    await installer.install(lock, { beforeReplace: async () => { calls.push('release-owner') } })

    expect(calls.indexOf('release-owner')).toBeGreaterThan(calls.indexOf('verify:gog:com.pagis.gog:ABCDE12345'))
    expect(fs.readFileSync(installed, 'utf8')).toBe('server')
    expect(fs.existsSync(path.join(runtime, 'retained'))).toBe(false)
  })

  it('puts the damaged runtime back when the activation of its repair fails', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const installed = await new RuntimeInstaller(runtime, adapters(source, [])).install(lock)
    fs.writeFileSync(installed, 'tampered', { mode: 0o755 })
    const failed = adapters(source, [])
    failed.activate = async () => { throw new Error('activation failed') }

    await expect(new RuntimeInstaller(runtime, failed).install(lock, { beforeReplace: async () => {} }))
      .rejects.toThrow(/activation failed/)

    expect(fs.readFileSync(installed, 'utf8')).toBe('tampered')
    expect(fs.readdirSync(path.join(runtime, 'retained'))).toEqual([])
  })

  it('removes a damaged runtime that an interrupted repair retained at the next start', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const installer = new RuntimeInstaller(runtime, adapters(source, []))
    await installer.install(lock)
    const retained = path.join(runtime, 'retained', `${lock.release}-crashed`)
    fs.mkdirSync(retained, { recursive: true })
    fs.writeFileSync(path.join(retained, 'pagis'), 'tampered')

    await installer.install(lock)

    expect(fs.existsSync(path.join(runtime, 'retained'))).toBe(false)
  })

  it('keeps a damaged runtime when an external owner is still running', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const installer = new RuntimeInstaller(runtime, adapters(source, []))
    const installed = await installer.install(lock)
    fs.writeFileSync(installed, 'tampered', { mode: 0o755 })

    await expect(installer.install(lock, {
      beforeReplace: async () => { throw new Error('server is still running') },
    })).rejects.toThrow(/still running/)

    expect(fs.readFileSync(installed, 'utf8')).toBe('tampered')
  })

  it('uses a verified cached image while the release host is offline', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    fs.mkdirSync(path.join(runtime, 'downloads'), { recursive: true })
    fs.writeFileSync(path.join(runtime, 'downloads', `${lock.asset.sha256}.part`), 'fixture dmg')
    const calls: string[] = []
    const offline = adapters(source, calls)
    offline.download = async () => { throw new Error('offline') }

    await expect(new RuntimeInstaller(runtime, offline).install(lock)).resolves.toContain('/pagis')
    expect(calls).not.toContain('download')
  })

  it('re-downloads a corrupt cached image', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    fs.mkdirSync(path.join(runtime, 'downloads'), { recursive: true })
    fs.writeFileSync(path.join(runtime, 'downloads', `${lock.asset.sha256}.part`), 'corrupt')
    const calls: string[] = []

    await new RuntimeInstaller(runtime, adapters(source, calls)).install(lock)

    expect(calls.filter((call) => call === 'download')).toHaveLength(1)
  })

  it('reports each phase in order, with the received bytes of the download', async () => {
    const { root, lock, source } = fixture()
    const reporting = adapters(source, [])
    reporting.download = async (_url, destination, _size, _signal, onBytes) => {
      fs.writeFileSync(destination, 'fixture dmg')
      onBytes?.(4)
      onBytes?.(11)
    }
    const progress: string[] = []

    await new RuntimeInstaller(path.join(root, 'runtime'), reporting).install(lock, { onProgress: (step) => {
      progress.push(step.phase === 'downloading' ? `downloading:${step.received}/${step.total}` : step.phase)
    } })

    expect(progress).toEqual([
      'downloading:0/11',
      'downloading:4/11',
      'downloading:11/11',
      'verifying',
      'extracting',
      'activating',
    ])
  })

  it('reports no download for a verified cached image', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    fs.mkdirSync(path.join(runtime, 'downloads'), { recursive: true })
    fs.writeFileSync(path.join(runtime, 'downloads', `${lock.asset.sha256}.part`), 'fixture dmg')
    const progress: string[] = []

    await new RuntimeInstaller(runtime, adapters(source, [])).install(lock, { onProgress: (step) => progress.push(step.phase) })

    expect(progress).toEqual(['verifying', 'extracting', 'activating'])
  })

  it('reports only the check of a release that is already installed', async () => {
    const { root, lock, source } = fixture()
    const installer = new RuntimeInstaller(path.join(root, 'runtime'), adapters(source, []))
    await installer.install(lock)
    const progress: string[] = []

    await installer.install(lock, { onProgress: (step) => progress.push(step.phase) })

    expect(progress).toEqual(['verifying'])
  })

  it('reports missing assets, timeouts, no space and read-only storage as retryable failures', async () => {
    for (const [failure, message] of [
      [Object.assign(new Error('HTTP 404'), { code: 'ENOENT' }), /HTTP 404/],
      [new DOMException('timed out', 'TimeoutError'), /timed out/],
      [Object.assign(new Error('full'), { code: 'ENOSPC' }), /free disk space/],
      [Object.assign(new Error('read only'), { code: 'EROFS' }), /cannot write/],
    ] as const) {
      const { root, lock, source } = fixture()
      const failed = adapters(source, [])
      failed.download = async () => { throw failure }
      await expect(new RuntimeInstaller(path.join(root, 'runtime'), failed).install(lock)).rejects.toThrow(message)
    }
  })

  it('cancellation removes its partial download and preserves unrelated files', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const unrelated = path.join(runtime, 'staging', 'keep-me')
    fs.mkdirSync(unrelated, { recursive: true })
    fs.writeFileSync(path.join(unrelated, 'memory'), 'keep')
    const abort = new AbortController()
    const cancelled = adapters(source, [])
    cancelled.download = async (_url, destination, _size, signal) => {
      fs.writeFileSync(destination, 'partial')
      abort.abort()
      signal?.throwIfAborted()
    }

    await expect(new RuntimeInstaller(runtime, cancelled).install(lock, { signal: abort.signal })).rejects.toThrow(/cancelled/)

    expect(fs.existsSync(path.join(runtime, 'downloads', `${lock.asset.sha256}.part`))).toBe(false)
    expect(fs.readFileSync(path.join(unrelated, 'memory'), 'utf8')).toBe('keep')
  })

  it('keeps the previous active files when activation fails', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const failed = adapters(source, [])
    failed.activate = async () => { throw Object.assign(new Error('read only'), { code: 'EROFS' }) }

    await expect(new RuntimeInstaller(runtime, failed).install(lock)).rejects.toThrow(/cannot write/)

    expect(fs.existsSync(path.join(runtime, 'releases', lock.release, 'darwin-arm64'))).toBe(false)
    expect(JSON.parse(fs.readFileSync(path.join(runtime, 'install.json'), 'utf8'))).toMatchObject({ phase: 'activating' })
  })

  it('serializes two install requests for the same runtime', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const calls: string[] = []
    let releaseDownload!: () => void
    const blocked = adapters(source, calls)
    blocked.download = async (_url, destination) => {
      calls.push('download')
      await new Promise<void>((resolve) => { releaseDownload = resolve })
      fs.writeFileSync(destination, 'fixture dmg')
    }

    const first = new RuntimeInstaller(runtime, blocked).install(lock)
    while (!releaseDownload) await new Promise((resolve) => setTimeout(resolve, 1))
    const second = new RuntimeInstaller(runtime, blocked).install(lock)
    releaseDownload()
    await Promise.all([first, second])

    expect(calls.filter((call) => call === 'download')).toHaveLength(1)
  })

  it('records an interrupted phase without touching Workspace data', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const workspace = path.join(root, 'workspace')
    fs.mkdirSync(workspace)
    fs.writeFileSync(path.join(workspace, 'secrets.enc'), 'keep')
    fs.writeFileSync(path.join(workspace, 'pagis.db'), 'onboarded')
    fs.mkdirSync(path.join(workspace, 'memory'))
    fs.writeFileSync(path.join(workspace, 'memory', 'MEMORY.md'), 'keep memory')
    fs.mkdirSync(path.join(workspace, 'computer-volumes'))
    fs.writeFileSync(path.join(workspace, 'computer-volumes', 'volume'), 'keep volume')
    const failed = adapters(source, [])
    failed.mountReadOnly = async () => { throw new Error('image could not be mounted') }

    await expect(new RuntimeInstaller(runtime, failed).install(lock)).rejects.toThrow(/mounted/)

    expect(fs.readFileSync(path.join(workspace, 'secrets.enc'), 'utf8')).toBe('keep')
    expect(fs.readFileSync(path.join(workspace, 'pagis.db'), 'utf8')).toBe('onboarded')
    expect(fs.readFileSync(path.join(workspace, 'memory', 'MEMORY.md'), 'utf8')).toBe('keep memory')
    expect(fs.readFileSync(path.join(workspace, 'computer-volumes', 'volume'), 'utf8')).toBe('keep volume')
    expect(JSON.parse(fs.readFileSync(path.join(runtime, 'install.json'), 'utf8'))).toMatchObject({
      release: lock.release,
      phase: 'extracting',
    })
    expect(fs.readdirSync(path.join(runtime, 'staging'))).toEqual([])
  })

  it('detaches the exact interrupted mount before cleaning its staging tree', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const work = path.join(runtime, 'staging', 'install-crashed')
    fs.mkdirSync(path.join(work, 'mounted'), { recursive: true })
    fs.writeFileSync(path.join(runtime, 'install.json'), JSON.stringify({
      schema: 1, release: lock.release, phase: 'extracting', work: 'install-crashed',
    }))
    const calls: string[] = []
    const recovered = adapters(source, calls)
    recovered.recoverMount = async (mountpoint) => {
      expect(mountpoint).toBe(path.join(work, 'mounted'))
      calls.push('recover-mount')
    }

    await new RuntimeInstaller(runtime, recovered).install(lock)

    expect(calls[0]).toBe('recover-mount')
    expect(fs.existsSync(work)).toBe(false)
  })

  it('recovers owned staging from an older release before a forward update', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const oldPackage = path.join(runtime, 'releases', '0.1.0', 'darwin-arm64', 'pagis')
    fs.mkdirSync(path.dirname(oldPackage), { recursive: true })
    fs.writeFileSync(oldPackage, 'running package')
    const work = path.join(runtime, 'staging', 'install-old-client')
    fs.mkdirSync(path.join(work, 'mounted'), { recursive: true })
    fs.writeFileSync(path.join(runtime, 'install.json'), JSON.stringify({
      schema: 1, release: '0.1.0', phase: 'extracting', work: 'install-old-client',
    }))
    lock.release = '0.2.0'
    lock.asset.name = 'pagis-server-0.2.0-aarch64-apple-darwin.dmg'
    lock.asset.url = `https://github.com/pagis-co/pagis/releases/download/v0.2.0/${lock.asset.name}`
    const calls: string[] = []
    const recovered = adapters(source, calls)
    recovered.recoverMount = async (mountpoint) => {
      expect(mountpoint).toBe(path.join(work, 'mounted'))
      calls.push('recover-old-mount')
    }

    const installed = await new RuntimeInstaller(runtime, recovered).install(lock, {
      beforeReplace: async () => { throw new Error('the working old server must stay running') },
    })

    expect(installed).toContain(path.join('releases', '0.2.0', 'darwin-arm64', 'pagis'))
    expect(calls[0]).toBe('recover-old-mount')
    expect(fs.readFileSync(oldPackage, 'utf8')).toBe('running package')
    expect(fs.existsSync(work)).toBe(false)
  })

  it('rejects a symlink in every runtime-owned ancestor', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const outside = path.join(root, 'outside')
    fs.mkdirSync(runtime)
    fs.mkdirSync(outside)
    fs.symlinkSync(outside, path.join(runtime, 'releases'))

    await expect(new RuntimeInstaller(runtime, adapters(source, [])).install(lock)).rejects.toThrow(/safe runtime path/)
    expect(fs.readdirSync(outside)).toEqual([])
  })

  it('does not detach or clean through a symlinked interrupted mountpoint', async () => {
    const { root, lock, source } = fixture()
    const runtime = path.join(root, 'runtime')
    const work = path.join(runtime, 'staging', 'install-crashed')
    const outside = path.join(root, 'outside')
    fs.mkdirSync(work, { recursive: true })
    fs.mkdirSync(outside)
    fs.symlinkSync(outside, path.join(work, 'mounted'))
    fs.writeFileSync(path.join(runtime, 'install.json'), JSON.stringify({
      schema: 1, release: lock.release, phase: 'extracting', work: 'install-crashed',
    }))
    let detached = false
    const unsafe = adapters(source, [])
    unsafe.recoverMount = async () => { detached = true }

    await expect(new RuntimeInstaller(runtime, unsafe).install(lock)).rejects.toThrow(/safe runtime path/)

    expect(detached).toBe(false)
    expect(fs.existsSync(path.join(work, 'mounted'))).toBe(true)
  })
})

const LINUX_FILES = [
  ['pagis', 'server', 0o755, 'executable'],
  ['gog', 'gog', 0o755, 'executable'],
  ['LICENSE', 'pagis license', 0o644, 'file'],
  ['LICENSE.gog', 'license', 0o644, 'file'],
  ['THIRD_PARTY_NOTICES', 'notices', 0o644, 'file'],
] as const

/**
 * A real gzip tar archive of the five files, built with the system `tar`
 * as the release builds the server archive. `change` alters the
 * packed tree first, and the lock always matches the finished archive's
 * bytes, so a check that fails is the check under test and never the
 * outer hash.
 */
function linuxFixture(change?: (tree: string) => string[]): { root: string; lock: LinuxRuntimeLock; archive: string } {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-linux-installer-'))
  roots.push(root)
  const tree = path.join(root, 'tree')
  fs.mkdirSync(tree)
  for (const [name, body, mode] of LINUX_FILES) {
    fs.writeFileSync(path.join(tree, name), body)
    fs.chmodSync(path.join(tree, name), mode)
  }
  const members = change?.(tree) ?? LINUX_FILES.map(([name]) => name)
  const release = '0.1.0'
  const name = `pagis-server-${release}-x86_64-unknown-linux-gnu.tar.gz`
  const archive = path.join(root, name)
  execFileSync('tar', ['-czf', archive, '-C', tree, ...members], { env: { ...process.env, COPYFILE_DISABLE: '1' } })
  const bytes = fs.readFileSync(archive)
  return {
    root,
    archive,
    lock: {
      schema: 1,
      release,
      platform: 'linux',
      arch: 'x64',
      asset: {
        format: 'tar.gz',
        name,
        url: `https://github.com/pagis-co/pagis/releases/download/v${release}/${name}`,
        size: bytes.length,
        sha256: crypto.createHash('sha256').update(bytes).digest('hex'),
      },
      entries: LINUX_FILES.map(([file, body, mode, kind]) => ({
        path: file,
        kind,
        size: Buffer.byteLength(body),
        sha256: crypto.createHash('sha256').update(body).digest('hex'),
        mode,
      })),
      computer_image: `ghcr.io/pagis-co/pagis-computer@sha256:${'a'.repeat(64)}`,
    },
  }
}

function linuxAdapters(archive: string, calls: string[]): LinuxAdapters {
  return {
    platform: 'linux',
    download: async (_url, destination) => {
      calls.push('download')
      fs.copyFileSync(archive, destination)
    },
    activate: async (staged, destination) => { fs.renameSync(staged, destination) },
    extract: async (file, destination, names) => {
      calls.push('extract')
      await extractArchive(file, destination, names)
    },
  }
}

describe('the Linux server package', () => {
  it('extracts, verifies and installs only the locked files of the archive', async () => {
    const { root, lock, archive } = linuxFixture()
    const calls: string[] = []

    const installed = await new RuntimeInstaller(path.join(root, 'runtime'), linuxAdapters(archive, calls)).install(lock)

    expect(installed).toBe(path.join(root, 'runtime', 'releases', '0.1.0', 'linux-x64', 'pagis'))
    expect(fs.readFileSync(installed, 'utf8')).toBe('server')
    expect(fs.statSync(installed).mode & 0o7777).toBe(0o755)
    expect(fs.statSync(path.join(path.dirname(installed), 'LICENSE')).mode & 0o7777).toBe(0o644)
    expect(calls).toEqual(['download', 'extract'])
  })

  it('keeps the archive modes whatever the umask is', async () => {
    const { root, lock, archive } = linuxFixture()
    const previous = process.umask(0o077)
    try {
      const installed = await new RuntimeInstaller(path.join(root, 'runtime'), linuxAdapters(archive, [])).install(lock)
      expect(fs.statSync(installed).mode & 0o7777).toBe(0o755)
    } finally {
      process.umask(previous)
    }
  })

  it('refuses an archive with a member the lock does not name', async () => {
    const { root, lock, archive } = linuxFixture((tree) => {
      fs.writeFileSync(path.join(tree, 'surprise'), 'extra')
      return [...LINUX_FILES.map(([name]) => name), 'surprise']
    })

    await expect(new RuntimeInstaller(path.join(root, 'runtime'), linuxAdapters(archive, [])).install(lock))
      .rejects.toThrow(/surprise/)
    expect(fs.existsSync(path.join(root, 'runtime', 'releases', lock.release))).toBe(false)
  })

  it('refuses a link, a wrong mode and wrong bytes before activation', async () => {
    for (const change of [
      (tree: string) => {
        fs.rmSync(path.join(tree, 'pagis'))
        fs.symlinkSync('gog', path.join(tree, 'pagis'))
      },
      (tree: string) => fs.chmodSync(path.join(tree, 'pagis'), 0o775),
      (tree: string) => fs.writeFileSync(path.join(tree, 'gog'), 'changed'),
    ]) {
      const { root, lock, archive } = linuxFixture((tree) => {
        change(tree)
        return LINUX_FILES.map(([name]) => name)
      })

      await expect(new RuntimeInstaller(path.join(root, 'runtime'), linuxAdapters(archive, [])).install(lock)).rejects.toThrow()
      expect(fs.existsSync(path.join(root, 'runtime', 'releases', lock.release))).toBe(false)
    }
  })

  it('refuses changed archive bytes before it extracts anything', async () => {
    const { root, lock, archive } = linuxFixture()
    const calls: string[] = []
    const changed = linuxAdapters(archive, calls)
    changed.download = async (_url, destination) => { fs.writeFileSync(destination, 'changed') }

    await expect(new RuntimeInstaller(path.join(root, 'runtime'), changed).install(lock)).rejects.toThrow(/wrong size|wrong hash/)
    expect(calls).toEqual([])
  })

  it('starts a verified installed release offline and refuses one that changed', async () => {
    const { root, lock, archive } = linuxFixture()
    const runtime = path.join(root, 'runtime')
    const installed = await new RuntimeInstaller(runtime, linuxAdapters(archive, [])).install(lock)
    const offline = linuxAdapters(archive, [])
    offline.download = async () => { throw new Error('offline') }

    await expect(new RuntimeInstaller(runtime, offline).install(lock)).resolves.toBe(installed)

    fs.writeFileSync(installed, 'tampered')
    await expect(new RuntimeInstaller(runtime, offline).install(lock)).rejects.toThrow(/wrong size|wrong hash/)
  })

  it('refuses a lock of another platform', async () => {
    const { root, source } = fixture()
    const { lock } = linuxFixture()

    await expect(new RuntimeInstaller(path.join(root, 'runtime'), adapters(source, [])).install(lock))
      .rejects.toThrow(/darwin packages and cannot install a linux one/)
  })
})

/** A person with the Client App finds the `pagis` command for a backup
 *  from the Back up and restore page of the documentation site alone, so
 *  the page names the file where the installer puts it, under the
 *  `userData` directory of each platform. */
describe('the documented pagis command', () => {
  it('is where the installer puts it', async () => {
    const readme = fs.readFileSync(path.join(__dirname, '..', '..', 'docs-site', 'content', 'client-app', 'backup.mdx'), 'utf8')
    const darwin = fixture()
    const linux = linuxFixture()
    const installed = [
      ['~/Library/Application Support/Pagis', darwin.root, await new RuntimeInstaller(path.join(darwin.root, 'runtime'), adapters(darwin.source, [])).install(darwin.lock)],
      ['~/.config/Pagis', linux.root, await new RuntimeInstaller(path.join(linux.root, 'runtime'), linuxAdapters(linux.archive, [])).install(linux.lock)],
    ] as const

    for (const [userData, root, command] of installed) {
      const relative = path.relative(root, command).split(path.sep).join('/').replace('/0.1.0/', '/<release>/')
      expect(readme).toContain(`\`${userData}/${relative}\``)
    }
    expect(readme).toContain('`~/.config/Pagis/runtime/releases/<release>/linux-arm64/pagis`')
  })
})

interface Answer {
  status: number
  headers?: Record<string, string | undefined>
  body?: Buffer | string
}

/**
 * A local stand-in for the release host. It answers as
 * `python3 -m http.server` does: HTTP/1.0, and it closes the connection
 * after the body. `get` sends each request to it, and keeps the host
 * that the client asked for in the `Host` header.
 */
async function releaseHost(answer: (host: string, path: string) => Answer): Promise<{
  get: HttpsGet
  requests: string[]
  close(): Promise<void>
}> {
  const requests: string[] = []
  const server = net.createServer((socket) => {
    // The client destroys an answer that it refuses, for example a 404,
    // while the body is still in flight. The reset then reaches this
    // socket, and the release host ignores it as a real host does.
    socket.on('error', () => {})
    let head = ''
    socket.on('data', (chunk: Buffer) => {
      head += chunk.toString('latin1')
      if (!head.includes('\r\n\r\n')) return
      const [requestLine, ...lines] = head.split('\r\n')
      const target = requestLine.split(' ')[1]
      const host = lines.find((line) => line.toLowerCase().startsWith('host:'))?.slice(5).trim() ?? ''
      requests.push(`https://${host}${target}`)
      const { status, headers = {}, body = '' } = answer(host, target)
      const fields = Object.entries(headers).filter(([, value]) => value !== undefined).map(([name, value]) => `${name}: ${value}\r\n`).join('')
      socket.write(`HTTP/1.0 ${status} Answer\r\n${fields}\r\n`)
      socket.end(body)
    })
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = (server.address() as net.AddressInfo).port
  const get: HttpsGet = (url, options, respond) => http.get({
    host: '127.0.0.1',
    port,
    path: `${url.pathname}${url.search}`,
    headers: { ...options.headers, host: url.host },
    signal: options.signal,
  }, respond)
  return { get, requests, close: () => new Promise((resolve) => server.close(() => resolve())) }
}

const ASSET_URL = 'https://github.com/pagis-co/pagis/releases/download/v0.1.0/server.dmg'

describe('the bounded server download', () => {
  it('follows only a bounded GitHub HTTPS asset redirect', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-download-'))
    roots.push(root)
    const destination = path.join(root, 'asset.part')
    const host = await releaseHost((name) => name === 'github.com'
      ? { status: 302, headers: { location: 'https://release-assets.githubusercontent.com/final' } }
      : { status: 200, headers: { 'content-length': '6' }, body: 'locked' })

    try {
      await downloadLockedAsset(ASSET_URL, destination, 6, { get: host.get })
    } finally {
      await host.close()
    }

    expect(host.requests).toEqual([ASSET_URL, 'https://release-assets.githubusercontent.com/final'])
    expect(fs.readFileSync(destination, 'utf8')).toBe('locked')
  })

  it('completes a download whose server closes the connection after the last byte', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-download-'))
    roots.push(root)
    const destination = path.join(root, 'asset.part')
    const body = crypto.randomBytes(16 * 1024 * 1024)
    const host = await releaseHost(() => ({
      status: 200,
      headers: { 'content-length': String(body.length) },
      body,
    }))

    try {
      await downloadLockedAsset(ASSET_URL, destination, body.length, { get: host.get })
    } finally {
      await host.close()
    }

    expect(fs.readFileSync(destination).equals(body)).toBe(true)
  })

  it('reports the bytes it has received, up to the whole asset', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-download-'))
    roots.push(root)
    const body = crypto.randomBytes(1024 * 1024)
    const host = await releaseHost(() => ({
      status: 200,
      headers: { 'content-length': String(body.length) },
      body,
    }))
    const received: number[] = []

    try {
      await downloadLockedAsset(ASSET_URL, path.join(root, 'asset.part'), body.length, {
        get: host.get,
        onBytes: (bytes) => received.push(bytes),
      })
    } finally {
      await host.close()
    }

    expect(received.length).toBeGreaterThan(0)
    expect(received).toEqual([...received].sort((a, b) => a - b))
    expect(received.at(-1)).toBe(body.length)
  })

  it('rejects an external redirect, a wrong length and a body that ends early', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-download-'))
    roots.push(root)
    for (const [answer, message] of [
      [{ status: 302, headers: { location: 'https://evil.example/file' } }, /outside GitHub/],
      [{ status: 200, headers: { 'content-length': '7' }, body: 'lockedd' }, /length does not match/],
      [{ status: 200, headers: { 'content-length': '6' }, body: 'loc' }, /ended before/],
    ] as const) {
      const host = await releaseHost(() => answer)
      try {
        await expect(downloadLockedAsset(ASSET_URL, path.join(root, `${crypto.randomUUID()}.part`), 6, { get: host.get }))
          .rejects.toThrow(message)
      } finally {
        await host.close()
      }
    }
  })

  it('stops when setup is cancelled', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-download-'))
    roots.push(root)
    const host = await releaseHost(() => ({ status: 200, headers: { 'content-length': '6' }, body: 'locked' }))
    const abort = new AbortController()
    abort.abort(new Error('server installation was cancelled'))

    try {
      await expect(downloadLockedAsset(ASSET_URL, path.join(root, 'asset.part'), 6, { signal: abort.signal, get: host.get }))
        .rejects.toThrow(/cancelled/)
    } finally {
      await host.close()
    }
  })

  it('reports a missing release asset without creating a cache file', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-download-'))
    roots.push(root)
    const destination = path.join(root, 'missing.part')
    const host = await releaseHost(() => ({ status: 404, body: 'missing' }))

    try {
      await expect(downloadLockedAsset(ASSET_URL, destination, 6, { get: host.get })).rejects.toThrow(/HTTP 404/)
    } finally {
      await host.close()
    }

    expect(fs.existsSync(destination)).toBe(false)
  })

  it('keeps the Client Credential and the sign-in link out of a failure message', () => {
    const message = installationError(new Error(
      `client-credential=${'a'.repeat(64)} at ` +
      'http://127.0.0.1:4400/api/v1/sessions/link/do-not-print with pagis_session=do-not-print',
    )).message

    expect(message).toContain('credential=[redacted]')
    expect(message).toContain('/api/v1/sessions/link/[redacted]')
    expect(message).toContain('pagis_session=[redacted]')
    expect(message).not.toContain('do-not-print')
    expect(message).not.toContain('a'.repeat(64))
  })
})
