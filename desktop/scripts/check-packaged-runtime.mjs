import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { createRequire } from 'node:module'

// Run the compiled installer from the packaged app archive against a
// deterministic local release of this host's platform: a disk image
// through fixture adapters on macOS, and a real gzip tar archive that the
// installer extracts with the system `tar` on Linux.
const require = createRequire(import.meta.url)
const asar = require('@electron/asar')
const app = process.argv[2]
if (!app) throw new Error('usage: node scripts/check-packaged-runtime.mjs <app>')
const resources = app.endsWith('.app') ? path.join(app, 'Contents', 'Resources') : path.join(app, 'resources')

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-packaged-runtime-'))
try {
  const unpacked = path.join(root, 'app')
  asar.extractAll(path.join(resources, 'app.asar'), unpacked)
  const { RuntimeInstaller, extractArchive, installationError } = require(path.join(unpacked, 'dist', 'runtimeInstaller.js'))
  const { SetupCoordinator } = require(path.join(unpacked, 'dist', 'setupCoordinator.js'))

  const tree = path.join(root, 'fixture-tree')
  fs.mkdirSync(tree)
  const files = [
    ['pagis', 'server', 0o755, 'executable', 'co.pagis.server'],
    ['gog', 'helper', 0o755, 'executable', 'co.pagis.gog'],
    ['LICENSE', 'pagis license', 0o644, 'file', undefined],
    ['LICENSE.gog', 'license', 0o644, 'file', undefined],
    ['THIRD_PARTY_NOTICES', 'notices', 0o644, 'file', undefined],
  ]
  for (const [name, body, mode] of files) {
    fs.writeFileSync(path.join(tree, name), body)
    fs.chmodSync(path.join(tree, name), mode)
  }
  const linux = process.platform === 'linux'
  const entries = files.map(([name, body, mode, kind, codesign_id]) => ({
    path: name,
    kind,
    ...(codesign_id && !linux ? { codesign_id } : {}),
    size: Buffer.byteLength(body),
    sha256: crypto.createHash('sha256').update(body).digest('hex'),
    mode,
  }))
  const release = '1.2.3'
  const triple = process.arch === 'x64' ? 'x86_64-unknown-linux-gnu' : 'aarch64-unknown-linux-gnu'
  const name = linux ? `pagis-server-${release}-${triple}.tar.gz` : `pagis-server-${release}-aarch64-apple-darwin.dmg`
  const asset = path.join(root, name)
  if (linux) execFileSync('tar', ['-czf', asset, '-C', tree, ...files.map(([file]) => file)])
  else fs.writeFileSync(asset, 'deterministic local release fixture')
  const assetBytes = fs.readFileSync(asset)
  const lock = {
    schema: 1,
    release,
    platform: linux ? 'linux' : 'darwin',
    arch: linux ? process.arch : 'arm64',
    asset: {
      format: linux ? 'tar.gz' : 'dmg',
      name,
      url: `https://github.com/pagis-co/pagis/releases/download/v${release}/${name}`,
      size: assetBytes.length,
      sha256: crypto.createHash('sha256').update(assetBytes).digest('hex'),
      ...(linux ? {} : { team_id: 'ABCDE12345' }),
    },
    entries,
    computer_image: `ghcr.io/pagis-co/pagis-computer@sha256:${'a'.repeat(64)}`,
  }

  let downloads = 0
  const download = async (_url, destination) => { downloads += 1; fs.copyFileSync(asset, destination) }
  const activate = async (staged, destination) => { fs.renameSync(staged, destination) }
  const quarantine = new Map()
  const adapters = linux
    ? { platform: 'linux', download, activate, extract: extractArchive }
    : {
        platform: 'darwin',
        download,
        activate,
        async recoverMount() {},
        async mountReadOnly(_dmg, mountpoint) {
          for (const [file, _body, mode] of files) {
            fs.copyFileSync(path.join(tree, file), path.join(mountpoint, file))
            fs.chmodSync(path.join(mountpoint, file), mode)
          }
          return async () => {}
        },
        async verifyImage() {},
        async quarantine(file, value) {
          if (value !== undefined) quarantine.set(file, value)
          return value ?? quarantine.get(file) ?? '0083;fixture;Pagis;fixture'
        },
        async verifyExecutable() {},
      }
  const runtime = path.join(root, 'client-state', 'runtime')
  const installer = new RuntimeInstaller(runtime, adapters)
  const binary = await installer.install(lock)
  assert.equal(fs.readFileSync(binary, 'utf8'), 'server')
  assert.equal(fs.statSync(binary).mode & 0o7777, 0o755)
  assert.equal(downloads, 1)

  adapters.download = async () => { throw new Error('network offline') }
  assert.equal(await installer.install(lock), binary)
  assert.equal(downloads, 1, 'the verified installed package must work offline')

  const secondRuntime = path.join(root, 'second-client-state', 'runtime')
  fs.mkdirSync(path.join(secondRuntime, 'downloads'), { recursive: true })
  fs.copyFileSync(asset, path.join(secondRuntime, 'downloads', `${lock.asset.sha256}.part`))
  const cached = new RuntimeInstaller(secondRuntime, adapters)
  assert.match(await cached.install(lock), /pagis$/)

  assert.match(installationError(Object.assign(new Error('full'), { code: 'ENOSPC' })).message, /free disk space/)
  const coordinator = new SetupCoordinator({
    install: async () => binary,
    start: async () => 'http://127.0.0.1:4400/',
    activate: () => {},
    openProduct: async () => {},
    connect: async () => 'https://pagis.example.com/',
    openRemoteAccessSwitch: async () => {},
  })
  await assert.rejects(coordinator.run({ kind: 'server' }), /Enter the address of your Pagis server/)
  await assert.rejects(coordinator.run({ kind: 'local' }), /Pagis does not know this setup/)
  await assert.rejects(coordinator.run({ kind: 'local', people: 'one', url: 'https://remote.example' }), /Pagis does not know this setup/)

  console.log(`pagis packaged runtime: ${lock.platform} install, offline cache, storage error and mode checks passed`)
} finally {
  fs.rmSync(root, { recursive: true, force: true })
}
