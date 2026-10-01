import semver from 'semver'

const SHA256 = /^[0-9a-f]{64}$/
const TEAM_ID = /^[A-Z0-9]+$/
const IMAGE = /^ghcr\.io\/pagis-co\/pagis-computer@sha256:[0-9a-f]{64}$/
const RELEASES = 'https://github.com/pagis-co/pagis/releases/download'

/** A file of the macOS package. An executable carries the code identifier
 *  its signature binds to the team in the lock. */
export type DarwinEntry =
  | { path: string; kind: 'executable'; codesign_id: string; size: number; sha256: string; mode: number }
  | { path: string; kind: 'file'; size: number; sha256: string; mode: number }

/** A file of the Linux package. Linux has no platform code signature, so
 *  the size, the hash and the mode are the whole identity of a file. */
export interface LinuxEntry {
  path: string
  kind: 'executable' | 'file'
  size: number
  sha256: string
  mode: number
}

export type LockedEntry = DarwinEntry | LinuxEntry

interface LockedAsset {
  name: string
  url: string
  size: number
  sha256: string
}

/** The macOS lock names a signed, notarized disk image and its team. */
export interface DarwinRuntimeLock {
  schema: 1
  release: string
  platform: 'darwin'
  arch: 'arm64'
  asset: LockedAsset & { format: 'dmg'; team_id: string }
  entries: DarwinEntry[]
  computer_image: string
}

/** The Linux lock names the server archive of its architecture. */
export interface LinuxRuntimeLock {
  schema: 1
  release: string
  platform: 'linux'
  arch: 'x64' | 'arm64'
  asset: LockedAsset & { format: 'tar.gz' }
  entries: LinuxEntry[]
  computer_image: string
}

export type RuntimeLock = DarwinRuntimeLock | LinuxRuntimeLock

/** The platform and architecture the client runs on, with Node's names. */
export interface ClientPlatform {
  platform: string
  arch: string
}

export const thisPlatform: ClientPlatform = { platform: process.platform, arch: process.arch }

const layout: Array<{ path: string; kind: LockedEntry['kind']; codesignId?: string; mode: number }> = [
  { path: 'pagis', kind: 'executable', codesignId: 'com.pagis.server', mode: 0o755 },
  { path: 'gog', kind: 'executable', codesignId: 'com.pagis.gog', mode: 0o755 },
  { path: 'LICENSE', kind: 'file', mode: 0o644 },
  { path: 'LICENSE.gog', kind: 'file', mode: 0o644 },
  { path: 'THIRD_PARTY_NOTICES', kind: 'file', mode: 0o644 },
]

/** The Rust target triple of each Linux architecture, which names the
 *  server archive. */
const LINUX_TRIPLES = { x64: 'x86_64-unknown-linux-gnu', arm64: 'aarch64-unknown-linux-gnu' } as const

/** The file name of the Runtime Lock of one platform in the release. */
export function runtimeLockFile(target: ClientPlatform): string {
  return `runtime-lock-${target.platform}-${target.arch}.json`
}

/** The URL below which the GitHub release `release` serves its assets. */
export function releaseUrl(release: string): string {
  return `${RELEASES}/v${release}`
}

/** The URL of the asset `name` of the GitHub release `release`. */
export function releaseAssetUrl(release: string, name: string): string {
  return `${releaseUrl(release)}/${name}`
}

/** The client carries no Runtime Lock it can use: a build from source
 *  with no lock, or a lock that does not parse for this client. The
 *  message names no file; `detail` holds the cause for the log. */
export class NoRuntimeLockError extends Error {
  constructor(readonly detail: string) {
    super(
      'This build of the Client App has no Server Runtime to install. Connect to a server, or use a released Client App.',
    )
    this.name = 'NoRuntimeLockError'
  }
}

/** Read and check the lock at `file`. Any failure is a
 *  NoRuntimeLockError. */
export function readRuntimeLock(
  file: string,
  clientVersion: string,
  read: (file: string) => string,
  host: ClientPlatform = thisPlatform,
): RuntimeLock {
  let text: string
  try {
    text = read(file)
  } catch (error) {
    throw new NoRuntimeLockError(`${file}: ${error instanceof Error ? error.message : String(error)}`)
  }
  try {
    return parseRuntimeLock(text, clientVersion, host)
  } catch (error) {
    throw new NoRuntimeLockError(`${file}: ${error instanceof Error ? error.message : String(error)}`)
  }
}

export function parseRuntimeLock(
  text: string,
  clientVersion: string,
  host: ClientPlatform = thisPlatform,
): RuntimeLock {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    throw new Error('the runtime lock is not valid JSON')
  }
  const root = record(value, 'runtime lock')
  exactFields(root, ['schema', 'release', 'platform', 'arch', 'asset', 'entries', 'computer_image'])
  if (root.schema !== 1) throw new Error('the runtime lock has an unsupported schema')
  if (root.platform !== host.platform || root.arch !== host.arch) {
    throw new Error(`the runtime lock does not name this ${host.platform} ${host.arch} client`)
  }
  if (root.release !== clientVersion) {
    throw new Error(`runtime lock release must equal client release ${clientVersion}`)
  }
  const release = string(root.release, 'release')
  if (semver.valid(release) === null) throw new Error('runtime lock release must be valid SemVer')
  const computerImage = string(root.computer_image, 'computer_image')
  if (!IMAGE.test(computerImage)) throw new Error('computer_image is not an immutable Pagis digest')
  const assetValue = record(root.asset, 'asset')

  if (root.platform === 'darwin' && root.arch === 'arm64') {
    exactFields(assetValue, ['format', 'name', 'url', 'size', 'sha256', 'team_id'])
    const asset = lockedAsset(assetValue, 'dmg', `pagis-server-${release}-aarch64-apple-darwin.dmg`, release)
    const teamId = string(assetValue.team_id, 'asset.team_id')
    if (!TEAM_ID.test(teamId)) throw new Error('asset.team_id is invalid')
    return {
      schema: 1,
      release,
      platform: 'darwin',
      arch: 'arm64',
      asset: { ...asset, format: 'dmg', team_id: teamId },
      entries: entries(root.entries, true) as DarwinEntry[],
      computer_image: computerImage,
    }
  }
  if (root.platform === 'linux' && (root.arch === 'x64' || root.arch === 'arm64')) {
    exactFields(assetValue, ['format', 'name', 'url', 'size', 'sha256'])
    const name = `pagis-server-${release}-${LINUX_TRIPLES[root.arch]}.tar.gz`
    return {
      schema: 1,
      release,
      platform: 'linux',
      arch: root.arch,
      asset: { ...lockedAsset(assetValue, 'tar.gz', name, release), format: 'tar.gz' },
      entries: entries(root.entries, false),
      computer_image: computerImage,
    }
  }
  throw new Error(`the runtime lock names the unsupported platform ${String(root.platform)} ${String(root.arch)}`)
}

function lockedAsset(value: Record<string, unknown>, format: string, expectedName: string, release: string): LockedAsset {
  const expectedUrl = releaseAssetUrl(release, expectedName)
  if (value.format !== format || value.name !== expectedName || value.url !== expectedUrl) {
    throw new Error(`runtime lock asset does not name the exact release ${format} package`)
  }
  return {
    name: expectedName,
    url: expectedUrl,
    size: positiveInteger(value.size, 'asset.size'),
    sha256: hash(value.sha256, 'asset.sha256'),
  }
}

function entries(value: unknown, signed: boolean): LockedEntry[] {
  if (!Array.isArray(value) || value.length !== layout.length) {
    throw new Error('runtime lock entries do not match the package layout')
  }
  const seen = new Set<string>()
  return value.map((raw, index): LockedEntry => {
    const entry = record(raw, `entries[${index}]`)
    const path = string(entry.path, `entries[${index}].path`)
    const spec = layout.find((candidate) => candidate.path === path)
    if (!spec || seen.has(path)) throw new Error('runtime lock entries do not match the package layout')
    seen.add(path)
    const codesignId = signed ? spec.codesignId : undefined
    exactFields(entry, codesignId
      ? ['path', 'kind', 'codesign_id', 'size', 'sha256', 'mode']
      : ['path', 'kind', 'size', 'sha256', 'mode'])
    if (entry.kind !== spec.kind || entry.mode !== spec.mode || entry.codesign_id !== codesignId) {
      throw new Error(`runtime lock entry ${path} has the wrong kind, mode or code identity`)
    }
    const common = {
      path,
      size: positiveInteger(entry.size, `${path}.size`),
      sha256: hash(entry.sha256, `${path}.sha256`),
      mode: spec.mode,
    }
    return codesignId
      ? { ...common, kind: 'executable', codesign_id: codesignId }
      : { ...common, kind: spec.kind }
  })
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${name} must be an object`)
  }
  return value as Record<string, unknown>
}

function exactFields(value: Record<string, unknown>, fields: string[]): void {
  const actual = Object.keys(value).sort()
  const expected = [...fields].sort()
  if (actual.length !== expected.length || actual.some((field, index) => field !== expected[index])) {
    throw new Error('runtime lock has a missing or unexpected field')
  }
}

function string(value: unknown, name: string): string {
  if (typeof value !== 'string' || value.length === 0) throw new Error(`${name} must be a string`)
  return value
}

function positiveInteger(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value) || (value as number) <= 0) throw new Error(`${name} must be a positive integer`)
  return value as number
}

function hash(value: unknown, name: string): string {
  const result = string(value, name)
  if (!SHA256.test(result)) throw new Error(`${name} must be a lowercase sha256`)
  return result
}
