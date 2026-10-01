import { execFile } from 'node:child_process'
import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as http from 'node:http'
import * as https from 'node:https'
import * as path from 'node:path'
import { Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { promisify } from 'node:util'

import type { LockedEntry, RuntimeLock } from './runtimeLock'

const exec = promisify(execFile)

interface CommonAdapters {
  /** Download the asset, and report the bytes received so far to
   *  `onBytes` as they arrive. */
  download(
    url: string,
    destination: string,
    maxBytes: number,
    signal?: AbortSignal,
    onBytes?: (received: number) => void,
  ): Promise<void>
  activate(staged: string, destination: string): Promise<void>
}

/**
 * The macOS package is a signed, notarized disk image. The client mounts
 * it read-only, keeps the download quarantine on every executable it
 * copies, and checks each code signature against the lock.
 */
export interface DarwinAdapters extends CommonAdapters {
  platform: 'darwin'
  recoverMount(mountpoint: string): Promise<void>
  mountReadOnly(dmg: string, mountpoint: string): Promise<() => Promise<void>>
  verifyImage(dmg: string, teamId: string): Promise<void>
  quarantine(file: string, value?: string): Promise<string>
  verifyExecutable(file: string, identifier: string, teamId: string): Promise<void>
}

/**
 * The Linux package is a gzip tar archive. Linux has no platform
 * notary or code signature, so the signed client's lock is the whole
 * check: the archive's size and hash, then each file's size, hash and
 * mode.
 */
export interface LinuxAdapters extends CommonAdapters {
  platform: 'linux'
  /** Extract exactly `names` from `archive` into `destination`, and
   *  refuse an archive that holds any other member. */
  extract(archive: string, destination: string, names: string[]): Promise<void>
}

export type RuntimeInstallerAdapters = DarwinAdapters | LinuxAdapters

type InstallPhase = 'downloading' | 'verifying' | 'extracting' | 'activating'

/** The phase that an installation enters. A download also gives the
 *  bytes received of the locked size. */
export type InstallProgress =
  | { phase: 'downloading'; received: number; total: number }
  | { phase: Exclude<InstallPhase, 'downloading'> }

interface InstallState {
  schema: 1
  release: string
  phase: InstallPhase
  work: string | null
}

export interface InstallOptions {
  signal?: AbortSignal
  beforeReplace?: () => Promise<void>
  onProgress?: (progress: InstallProgress) => void
}

const installQueues = new Map<string, Promise<void>>()

export class RuntimeInstaller {
  constructor(
    private readonly root: string,
    private readonly adapters: RuntimeInstallerAdapters = platformAdapters(),
  ) {}

  install(lock: RuntimeLock, options: InstallOptions = {}): Promise<string> {
    return serialized(this.root, () => this.installLocked(lock, options)).catch((error: unknown) => {
      throw installationError(error)
    })
  }

  private async installLocked(lock: RuntimeLock, options: InstallOptions): Promise<string> {
    const signal = options.signal
    const report = options.onProgress ?? (() => {})
    throwIfCancelled(signal)
    if (lock.platform !== this.adapters.platform) {
      throw new Error(`this client installs ${this.adapters.platform} packages and cannot install a ${lock.platform} one`)
    }
    requireOwnedDirectory(this.root, this.root)
    await this.reconcile(lock)
    // A damaged tree that a repair retained and did not remove, because
    // the client stopped before the repair ended, has no use any more.
    this.removeRetained()
    const installRoot = this.releaseRoot(lock)
    requireSafePath(this.root, installRoot)
    let replaceInstalled = false
    if (fs.existsSync(installRoot)) {
      report({ phase: 'verifying' })
      try {
        await this.verifyTree(installRoot, lock, undefined)
        this.clearState()
        return path.join(installRoot, 'pagis')
      } catch (error) {
        if (!options.beforeReplace) throw error
        replaceInstalled = true
      }
    }

    const downloads = path.join(this.root, 'downloads')
    const staging = path.join(this.root, 'staging')
    requireOwnedDirectory(this.root, downloads)
    requireOwnedDirectory(this.root, staging)
    const asset = path.join(downloads, `${lock.asset.sha256}.part`)
    if (!(await matchesFile(asset, lock.asset.size, lock.asset.sha256))) {
      removeOwnedFile(asset)
      this.writeState(lock.release, 'downloading', null)
      const total = lock.asset.size
      report({ phase: 'downloading', received: 0, total })
      try {
        await this.adapters.download(lock.asset.url, asset, total, signal, (received) => {
          report({ phase: 'downloading', received, total })
        })
      } catch (error) {
        removeOwnedFile(asset)
        throw installationError(error)
      }
    }
    throwIfCancelled(signal)
    this.writeState(lock.release, 'verifying', null)
    report({ phase: 'verifying' })
    await requireFile(asset, lock.asset.size, lock.asset.sha256, 'downloaded server package')
    const quarantine = await this.admitDownload(asset, lock)
    throwIfCancelled(signal)

    const work = fs.mkdtempSync(path.join(staging, 'install-'))
    const workName = path.basename(work)
    this.writeState(lock.release, 'extracting', workName)
    report({ phase: 'extracting' })
    // The package opens here: the macOS disk image mounts read-only, and
    // the Linux archive extracts. The installer then copies only the
    // locked files out of it.
    const mountpoint = path.join(work, 'mounted')
    const copied = path.join(work, 'release')
    fs.mkdirSync(mountpoint)
    fs.mkdirSync(copied)
    let detach: (() => Promise<void>) | undefined
    try {
      detach = await this.openPackage(asset, mountpoint, lock)
      throwIfCancelled(signal)
      await this.copyTree(mountpoint, copied, lock, quarantine, signal)
      await detach()
      detach = undefined
      throwIfCancelled(signal)
      this.writeState(lock.release, 'activating', workName)
      report({ phase: 'activating' })
      requireOwnedDirectory(this.root, path.dirname(installRoot))
      let retainedInstalled: string | null = null
      if (replaceInstalled) {
        await options.beforeReplace?.()
        throwIfCancelled(signal)
        const retained = path.join(this.root, 'retained')
        requireOwnedDirectory(this.root, retained)
        retainedInstalled = path.join(retained, `${lock.release}-${crypto.randomUUID()}`)
        fs.renameSync(installRoot, retainedInstalled)
      }
      try {
        await this.adapters.activate(copied, installRoot)
      } catch (error) {
        if (retainedInstalled && !fs.existsSync(installRoot)) fs.renameSync(retainedInstalled, installRoot)
        throw error
      }
      // The repair can roll back only until its replacement is active,
      // so the damaged tree goes now.
      this.removeRetained()
      throwIfCancelled(signal)
      this.clearState()
      return path.join(installRoot, 'pagis')
    } finally {
      if (detach) await detach()
      removeOwnedTree(this.root, staging, workName)
    }
  }

  /**
   * The platform check of a download whose bytes already match the lock.
   * On macOS it returns the download's quarantine, which every copied
   * executable must keep, after the disk image's own signature passes.
   * Linux has no such check, so the lock's hash is the whole admission.
   */
  private async admitDownload(asset: string, lock: RuntimeLock): Promise<string> {
    if (this.adapters.platform === 'linux' || lock.platform === 'linux') return ''
    const quarantine = await this.adapters.quarantine(asset)
    if (quarantine.trim() === '') throw new Error('the downloaded server image has no quarantine')
    await this.adapters.verifyImage(asset, lock.asset.team_id)
    return quarantine
  }

  private async openPackage(asset: string, directory: string, lock: RuntimeLock): Promise<() => Promise<void>> {
    if (this.adapters.platform === 'darwin') return this.adapters.mountReadOnly(asset, directory)
    await this.adapters.extract(asset, directory, lock.entries.map((entry) => entry.path))
    return async () => {}
  }

  private async reconcile(lock: RuntimeLock): Promise<void> {
    const state = this.readState()
    if (!state) return
    if (state.work !== null) {
      const staging = path.join(this.root, 'staging')
      const work = path.join(staging, state.work)
      requireSafePath(this.root, work)
      const mountpoint = path.join(work, 'mounted')
      requireSafePath(this.root, mountpoint)
      if (this.adapters.platform === 'darwin') await this.adapters.recoverMount(mountpoint)
      removeOwnedTree(this.root, staging, state.work)
    }
    // install.json owns only temporary work. A new signed client can retire
    // the old operation without touching its installed release or the
    // Workspace launch boundary. A verified content-addressed download stays
    // available for its matching lock.
    if (state.release !== lock.release) this.clearState()
  }

  /** Remove the damaged release trees that a repair moved aside. */
  private removeRetained(): void {
    const retained = path.join(this.root, 'retained')
    requireSafePath(this.root, retained)
    fs.rmSync(retained, { recursive: true, force: true })
  }

  private stateFile(): string { return path.join(this.root, 'install.json') }

  private writeState(release: string, phase: InstallPhase, work: string | null): void {
    writeAtomic(this.stateFile(), { schema: 1, release, phase, work })
  }

  private clearState(): void { fs.rmSync(this.stateFile(), { force: true }) }

  private readState(): InstallState | null {
    const file = this.stateFile()
    if (!fs.existsSync(file)) return null
    const metadata = fs.lstatSync(file)
    if (!metadata.isFile() || metadata.isSymbolicLink()) throw new Error('install.json is invalid; Repair needs a newer client')
    let value: unknown
    try { value = JSON.parse(fs.readFileSync(file, 'utf8')) } catch { throw new Error('install.json is invalid; Repair needs a newer client') }
    if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new Error('install.json is invalid; Repair needs a newer client')
    const record = value as Record<string, unknown>
    const fields = Object.keys(record).sort().join(',')
    const phases: InstallPhase[] = ['downloading', 'verifying', 'extracting', 'activating']
    if (
      fields !== 'phase,release,schema,work' || record.schema !== 1 ||
      typeof record.release !== 'string' || !phases.includes(record.phase as InstallPhase) ||
      (record.work !== null && (typeof record.work !== 'string' || !/^install-[A-Za-z0-9_-]+$/.test(record.work)))
    ) throw new Error('install.json is invalid; Repair needs a newer client')
    return record as unknown as InstallState
  }

  private releaseRoot(lock: RuntimeLock): string {
    return path.join(this.root, 'releases', lock.release, `${lock.platform}-${lock.arch}`)
  }

  private async copyTree(
    mounted: string,
    destination: string,
    lock: RuntimeLock,
    quarantine: string,
    signal?: AbortSignal,
  ): Promise<void> {
    requireExactNames(mounted, lock.entries)
    for (const entry of lock.entries) {
      const source = path.join(mounted, entry.path)
      const target = path.join(destination, entry.path)
      throwIfCancelled(signal)
      await requireRegularEntry(source, entry)
      await copyLockedFile(source, target, entry, signal)
      fs.chmodSync(target, entry.mode)
      if (this.adapters.platform === 'darwin' && lock.platform === 'darwin' && 'codesign_id' in entry) {
        await this.adapters.quarantine(target, quarantine)
        await this.adapters.verifyExecutable(target, entry.codesign_id, lock.asset.team_id)
      }
    }
  }

  private async verifyTree(
    root: string,
    lock: RuntimeLock,
    quarantine: string | undefined,
  ): Promise<void> {
    requireExactNames(root, lock.entries)
    for (const entry of lock.entries) {
      const file = path.join(root, entry.path)
      await requireRegularEntry(file, entry)
      await requireFile(file, entry.size, entry.sha256, entry.path)
      if (this.adapters.platform === 'darwin' && lock.platform === 'darwin' && 'codesign_id' in entry) {
        const found = await this.adapters.quarantine(file)
        if (found.trim() === '') throw new Error(`${entry.path} has no quarantine`)
        if (quarantine !== undefined && found !== quarantine) {
          throw new Error(`${entry.path} did not preserve quarantine`)
        }
        await this.adapters.verifyExecutable(file, entry.codesign_id, lock.asset.team_id)
      }
    }
  }
}

function requireExactNames(root: string, entries: LockedEntry[]): void {
  const actual = fs.readdirSync(root).sort()
  const expected = entries.map((entry) => entry.path).sort()
  if (actual.length !== expected.length || actual.some((name, index) => name !== expected[index])) {
    const unexpected = actual.filter((name) => !expected.includes(name))
    const missing = expected.filter((name) => !actual.includes(name))
    throw new Error(`server package layout has missing [${missing.join(', ')}] and unexpected [${unexpected.join(', ')}] entries`)
  }
}

async function requireRegularEntry(file: string, entry: LockedEntry): Promise<void> {
  const metadata = fs.lstatSync(file)
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1) {
    throw new Error(`${entry.path} is not one regular unlinked file`)
  }
  if ((metadata.mode & 0o7777) !== entry.mode) {
    throw new Error(`${entry.path} has the wrong mode`)
  }
}

async function copyLockedFile(source: string, destination: string, entry: LockedEntry, signal?: AbortSignal): Promise<void> {
  let size = 0
  const hasher = crypto.createHash('sha256')
  const input = fs.createReadStream(source)
  input.on('data', (chunk: Buffer) => {
    if (signal?.aborted) {
      input.destroy(signal.reason instanceof Error ? signal.reason : new Error('server installation was cancelled'))
      return
    }
    size += chunk.length
    if (size > entry.size) input.destroy(new Error(`${entry.path} exceeds its locked size`))
    hasher.update(chunk)
  })
  await pipeline(input, fs.createWriteStream(destination, { flags: 'wx', mode: entry.mode }))
  if (size !== entry.size) throw new Error(`${entry.path} has the wrong size`)
  if (hasher.digest('hex') !== entry.sha256) throw new Error(`${entry.path} has the wrong hash`)
}

async function matchesFile(file: string, size: number, sha256: string): Promise<boolean> {
  try {
    await requireFile(file, size, sha256, path.basename(file))
    return true
  } catch {
    return false
  }
}

async function requireFile(file: string, size: number, sha256: string, label: string): Promise<void> {
  const metadata = fs.lstatSync(file)
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size !== size) {
    throw new Error(`${label} has the wrong size or type`)
  }
  const actual = await hashFile(file, size)
  if (actual !== sha256) throw new Error(`${label} has the wrong hash`)
}

async function hashFile(file: string, maxBytes: number): Promise<string> {
  const hasher = crypto.createHash('sha256')
  let size = 0
  const input = fs.createReadStream(file)
  for await (const chunk of input) {
    size += (chunk as Buffer).length
    if (size > maxBytes) throw new Error(`${path.basename(file)} exceeds its locked size`)
    hasher.update(chunk as Buffer)
  }
  return hasher.digest('hex')
}

/**
 * Send one GET and answer the response head. The client uses Node's
 * `https.get`. A test sends the request to a local server.
 */
export type HttpsGet = (
  url: URL,
  options: https.RequestOptions,
  respond: (response: http.IncomingMessage) => void,
) => http.ClientRequest

export interface DownloadOptions {
  signal?: AbortSignal
  /** Called with the bytes received so far, as each chunk arrives. */
  onBytes?: (received: number) => void
  get?: HttpsGet
}

/**
 * Download the locked asset into `destination`. It reads the body with
 * Node's `https` module, which streams it into the file with
 * backpressure, and which also accepts a server that closes the
 * connection after the last byte (HTTP/1.0).
 */
export async function downloadLockedAsset(
  url: string,
  destination: string,
  maxBytes: number,
  { signal, onBytes, get = https.get }: DownloadOptions = {},
): Promise<void> {
  let current = new URL(url)
  if (current.protocol !== 'https:' || current.hostname !== 'github.com') {
    throw new Error('server download must start at the locked GitHub HTTPS URL')
  }
  const deadline = AbortSignal.timeout(10 * 60 * 1000)
  const abort = signal ? AbortSignal.any([signal, deadline]) : deadline
  try {
    for (let redirects = 0; ; redirects += 1) {
      const response = await request(get, current, abort)
      const status = response.statusCode ?? 0
      if (![301, 302, 303, 307, 308].includes(status)) {
        await save(response, destination, maxBytes, abort, onBytes)
        return
      }
      response.destroy()
      if (redirects === 3) throw new Error('server download redirected too many times')
      const location = response.headers.location
      if (!location) throw new Error('server download redirect has no location')
      const next = new URL(location, current)
      if (next.protocol !== 'https:' || !downloadHost(next.hostname)) {
        throw new Error('server download redirected outside GitHub release storage')
      }
      current = next
    }
  } catch (error) {
    // A cancel or the deadline aborts the request with an error of
    // Node's own. The reason of the signal says which it was.
    if (abort.aborted) throw abort.reason
    throw error
  }
}

function request(get: HttpsGet, url: URL, signal: AbortSignal): Promise<http.IncomingMessage> {
  return new Promise((resolve, reject) => {
    get(url, { signal, headers: { 'user-agent': 'Pagis' } }, resolve).on('error', reject)
  })
}

async function save(
  response: http.IncomingMessage,
  destination: string,
  maxBytes: number,
  signal: AbortSignal,
  onBytes?: (received: number) => void,
): Promise<void> {
  if (response.statusCode !== 200) {
    response.destroy()
    throw new Error(`server download failed with HTTP ${response.statusCode}`)
  }
  const length = response.headers['content-length']
  if (length === undefined || Number(length) !== maxBytes) {
    response.destroy()
    throw new Error('server download length does not match the Runtime Lock')
  }
  let size = 0
  const count = new Transform({
    transform(chunk: Buffer, _encoding, done) {
      size += chunk.length
      if (size > maxBytes) {
        done(new Error('server download exceeds the Runtime Lock size'))
        return
      }
      onBytes?.(size)
      done(null, chunk)
    },
  })
  try {
    await pipeline(response, count, fs.createWriteStream(destination, { flags: 'wx', mode: 0o600 }), { signal })
  } catch (error) {
    // Node ends a body that stops before its Content-Length with a
    // connection error.
    if (!signal.aborted && size < maxBytes && errorCode(error) === 'ECONNRESET') {
      throw new Error('server download ended before the Runtime Lock size')
    }
    throw error
  }
  if (size !== maxBytes) throw new Error('server download ended before the Runtime Lock size')
}

function errorCode(error: unknown): string {
  return typeof error === 'object' && error !== null && 'code' in error ? String((error as { code: unknown }).code) : ''
}

function downloadHost(hostname: string): boolean {
  return hostname === 'github.com' || hostname === 'release-assets.githubusercontent.com'
}

async function downloadQuarantined(
  url: string,
  destination: string,
  maxBytes: number,
  signal?: AbortSignal,
  onBytes?: (received: number) => void,
): Promise<void> {
  await downloadLockedAsset(url, destination, maxBytes, { signal, onBytes })
  const timestamp = Math.floor(Date.now() / 1000).toString(16)
  await command('/usr/bin/xattr', [
    '-w',
    'com.apple.quarantine',
    `0083;${timestamp};Pagis;${url}`,
    destination,
  ])
}

async function serialized<T>(root: string, operation: () => Promise<T>): Promise<T> {
  const key = path.resolve(root)
  const previous = installQueues.get(key) ?? Promise.resolve()
  let release!: () => void
  const turn = new Promise<void>((resolve) => { release = resolve })
  const queued = previous.catch(() => undefined).then(() => turn)
  installQueues.set(key, queued)
  await previous.catch(() => undefined)
  try {
    return await operation()
  } finally {
    release()
    if (installQueues.get(key) === queued) installQueues.delete(key)
  }
}

function requireOwnedDirectory(root: string, directory: string): void {
  const resolvedRoot = path.resolve(root)
  const resolvedDirectory = path.resolve(directory)
  if (resolvedDirectory !== resolvedRoot && !resolvedDirectory.startsWith(`${resolvedRoot}${path.sep}`)) {
    throw new Error(`${directory} escapes the runtime directory`)
  }
  if (!fs.existsSync(resolvedRoot)) fs.mkdirSync(resolvedRoot, { recursive: true, mode: 0o700 })
  const rootMetadata = fs.lstatSync(resolvedRoot)
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink()) throw new Error(`${root} is not a safe runtime directory`)
  let current = resolvedRoot
  const relative = path.relative(resolvedRoot, resolvedDirectory)
  for (const component of relative === '' ? [] : relative.split(path.sep)) {
    current = path.join(current, component)
    if (!fs.existsSync(current)) fs.mkdirSync(current, { mode: 0o700 })
    const metadata = fs.lstatSync(current)
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new Error(`${current} is not a safe runtime directory`)
  }
}

function requireSafePath(root: string, target: string): void {
  const resolvedRoot = path.resolve(root)
  const resolvedTarget = path.resolve(target)
  if (resolvedTarget !== resolvedRoot && !resolvedTarget.startsWith(`${resolvedRoot}${path.sep}`)) throw new Error(`${target} escapes the runtime directory`)
  let current = resolvedRoot
  for (const component of path.relative(resolvedRoot, resolvedTarget).split(path.sep)) {
    if (!component) continue
    current = path.join(current, component)
    if (!fs.existsSync(current)) return
    const metadata = fs.lstatSync(current)
    if (metadata.isSymbolicLink()) throw new Error(`${current} is not a safe runtime path`)
    if (current !== resolvedTarget && !metadata.isDirectory()) throw new Error(`${current} is not a safe runtime directory`)
  }
}

function removeOwnedFile(file: string): void {
  if (!fs.existsSync(file)) return
  const metadata = fs.lstatSync(file)
  if (!metadata.isFile() || metadata.isSymbolicLink()) throw new Error(`${file} is not an owned temporary file`)
  fs.rmSync(file)
}

function removeOwnedTree(root: string, parent: string, name: string): void {
  if (!/^install-[A-Za-z0-9_-]+$/.test(name)) throw new Error('install.json names an unsafe staging directory')
  if (!fs.existsSync(parent)) return
  requireSafePath(root, parent)
  const parentMetadata = fs.lstatSync(parent)
  if (!parentMetadata.isDirectory() || parentMetadata.isSymbolicLink()) throw new Error(`${parent} is not a safe staging directory`)
  const tree = path.join(parent, name)
  if (!fs.existsSync(tree)) return
  const metadata = fs.lstatSync(tree)
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new Error(`${tree} is not an owned staging directory`)
  fs.rmSync(tree, { recursive: true })
}

function writeAtomic(file: string, value: unknown): void {
  const temporary = `${file}.${crypto.randomUUID()}.tmp`
  fs.writeFileSync(temporary, `${JSON.stringify(value)}\n`, { flag: 'wx', mode: 0o600 })
  fs.renameSync(temporary, file)
}

function throwIfCancelled(signal?: AbortSignal): void {
  if (signal?.aborted) throw signal.reason instanceof Error ? signal.reason : new Error('server installation was cancelled')
}

export function installationError(error: unknown): Error {
  if (error instanceof DOMException && error.name === 'AbortError') return new Error('Server installation was cancelled. Select Retry to start again.')
  const code = typeof error === 'object' && error !== null && 'code' in error ? String((error as { code: unknown }).code) : ''
  if (code === 'ENOSPC') return new Error('The server needs more free disk space. Free space, then select Retry.')
  if (code === 'EROFS' || code === 'EACCES' || code === 'EPERM') return new Error('Pagis cannot write its server files. Check the permissions of its application data directory, then select Retry.')
  const message = error instanceof Error ? error.message : String(error)
  return new Error(message
    .replace(/(\/api\/v1\/sessions\/link\/)[^\s"'/?&]+/gi, '$1[redacted]')
    .replace(/(pagis_session=)[^;\s"']+/gi, '$1[redacted]')
    .replace(/("?(?:credential|token|api[_-]?key|secret)"?\s*[:=]\s*["']?)[^\s,"'}]+/gi, '$1[redacted]'))
}

async function command(program: string, args: string[]): Promise<string> {
  const result = await exec(program, args, { encoding: 'utf8', maxBuffer: 1024 * 1024 })
  return `${result.stdout}${result.stderr}`
}

/** The adapters of the platform this client runs on. */
export function platformAdapters(platform: string = process.platform): RuntimeInstallerAdapters {
  if (platform === 'darwin') return macAdapters
  if (platform === 'linux') return linuxAdapters
  throw new Error(`Pagis has no server package for ${platform}`)
}

/**
 * Extract the named members of a gzip tar archive with the system `tar`.
 * The archive's bytes already match the lock, and the member list must
 * equal `names` exactly before anything is written. `-p` keeps the
 * archive's modes whatever the umask is, so the mode check that follows
 * compares what the release packed.
 */
export async function extractArchive(archive: string, destination: string, names: string[]): Promise<void> {
  const listing = await exec('tar', ['-tzf', archive], { encoding: 'utf8', maxBuffer: 1024 * 1024 })
  const members = listing.stdout.split('\n').filter((line) => line !== '').sort()
  const expected = [...names].sort()
  if (members.length !== expected.length || members.some((name, index) => name !== expected[index])) {
    throw new Error(`the server archive holds [${members.join(', ')}], not the locked [${expected.join(', ')}]`)
  }
  await exec('tar', ['-xzpf', archive, '-C', destination, ...expected], { encoding: 'utf8', maxBuffer: 1024 * 1024 })
}

const linuxAdapters: LinuxAdapters = {
  platform: 'linux',
  download: (url, destination, maxBytes, signal, onBytes) =>
    downloadLockedAsset(url, destination, maxBytes, { signal, onBytes }),
  async activate(staged, destination) { fs.renameSync(staged, destination) },
  extract: extractArchive,
}

const macAdapters: DarwinAdapters = {
  platform: 'darwin',
  download: downloadQuarantined,
  async activate(staged, destination) { fs.renameSync(staged, destination) },
  async recoverMount(mountpoint) {
    if (!fs.existsSync(mountpoint)) return
    try {
      await command('/usr/bin/hdiutil', ['detach', mountpoint])
    } catch (error) {
      if (fs.readdirSync(mountpoint).length > 0) {
        throw new Error(`Pagis could not detach the interrupted server image: ${installationError(error).message}`)
      }
    }
  },
  async mountReadOnly(dmg, mountpoint) {
    await command('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mountpoint, dmg])
    return async () => { await command('/usr/bin/hdiutil', ['detach', mountpoint]) }
  },
  async verifyImage(dmg, teamId) {
    await command('/usr/bin/codesign', ['--verify', '--deep', '--strict', '--verbose=2', dmg])
    const details = await command('/usr/bin/codesign', ['-d', '--verbose=4', dmg])
    const foundTeam = details
      .split('\n')
      .find((line) => line.startsWith('TeamIdentifier='))
      ?.slice('TeamIdentifier='.length)
    if (foundTeam !== teamId) throw new Error('server image has the wrong signing team')
  },
  async quarantine(file, value) {
    if (value !== undefined) {
      await command('/usr/bin/xattr', ['-w', 'com.apple.quarantine', value, file])
      return value
    }
    return (await command('/usr/bin/xattr', ['-p', 'com.apple.quarantine', file])).trim()
  },
  async verifyExecutable(file, identifier, teamId) {
    const requirement = `identifier "${identifier}" and anchor apple generic and certificate leaf[subject.OU] = "${teamId}"`
    await command('/usr/bin/codesign', ['--verify', '--strict', '--verbose=2', `-R=${requirement}`, file])
  },
}
