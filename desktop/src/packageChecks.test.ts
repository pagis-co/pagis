// The release checks of the client package in `scripts/`: the inventory
// of the packaged app, and the Update feeds (ADR-0027).

import { execFileSync } from 'node:child_process'
import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { createPackage } from '@electron/asar'
import { afterEach, describe, expect, it } from 'vitest'

const SCRIPTS = path.join(__dirname, '..', 'scripts')

const roots: string[] = []
afterEach(() => {
  while (roots.length > 0) fs.rmSync(roots.pop()!, { recursive: true, force: true })
})

function temporary(): string {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-package-check-'))
  roots.push(root)
  return root
}

/** Run a check script, and answer its error output, or null when it passed. */
function check(script: string, args: string[], scripts = SCRIPTS, env = process.env): string | null {
  try {
    execFileSync(process.execPath, [path.join(scripts, script), ...args], { stdio: 'pipe', env })
    return null
  } catch (error) {
    return String((error as { stderr: Buffer }).stderr)
  }
}

/** A production dependency as `pnpm list --json` gives it, and the
 *  optional peers that its package.json declares. */
type Dependency = { version: string; optionalPeers?: string[]; dependencies?: Record<string, Dependency> }

/**
 * The production dependencies of the fixture app, as the real tree has
 * them. electron-updater needs another semver than the app, so the packed
 * app holds that semver below electron-updater. supports-color is an
 * optional peer of debug: pnpm lists it because a development dependency
 * resolves it, and electron-builder does not pack it.
 */
const PRODUCTION: Record<string, Dependency> = {
  'electron-updater': {
    version: '6.8.10',
    dependencies: {
      'builder-util-runtime': {
        version: '9.7.0',
        dependencies: {
          debug: {
            version: '4.4.3',
            optionalPeers: ['supports-color'],
            dependencies: {
              ms: { version: '2.1.3' },
              'supports-color': { version: '7.2.0', dependencies: { 'has-flag': { version: '4.0.0' } } },
            },
          },
          sax: { version: '1.6.1' },
        },
      },
      semver: { version: '7.7.4' },
    },
  },
  semver: { version: '7.8.5' },
}

/** The `node_modules` of the fixture app archive, as electron-builder
 *  writes it: the archive path of each package, and its version. */
const PACKED: Record<string, string> = {
  'node_modules/electron-updater': '6.8.10',
  'node_modules/electron-updater/node_modules/semver': '7.7.4',
  'node_modules/builder-util-runtime': '9.7.0',
  'node_modules/debug': '4.4.3',
  'node_modules/ms': '2.1.3',
  'node_modules/sax': '1.6.1',
  'node_modules/semver': '7.8.5',
}

function writePacked(source: string, packed: Record<string, string>) {
  for (const [directory, version] of Object.entries(packed)) {
    fs.mkdirSync(path.join(source, directory), { recursive: true })
    fs.writeFileSync(
      path.join(source, directory, 'package.json'),
      JSON.stringify({ name: path.posix.basename(directory), version }),
    )
  }
}

/**
 * An environment whose `pnpm` answers the production dependencies of the
 * Client App with `PRODUCTION`. It answers only the exact command of the
 * check, which runs at the repository root. Each package of the tree has
 * the `path` of its package.json, as pnpm gives it.
 */
function pnpmEnvironment(): NodeJS.ProcessEnv {
  const bin = temporary()
  const listed = (name: string, { version, optionalPeers = [], dependencies = {} }: Dependency): object => {
    const directory = path.join(bin, 'packages', `${name}@${version}`)
    fs.mkdirSync(directory, { recursive: true })
    fs.writeFileSync(path.join(directory, 'package.json'), JSON.stringify({
      name,
      version,
      peerDependencies: Object.fromEntries(optionalPeers.map((peer) => [peer, '*'])),
      peerDependenciesMeta: Object.fromEntries(optionalPeers.map((peer) => [peer, { optional: true }])),
    }))
    return { version, path: directory, dependencies: tree(dependencies) }
  }
  const tree = (dependencies: Record<string, Dependency>) =>
    Object.fromEntries(Object.entries(dependencies).map(([name, dependency]) => [name, listed(name, dependency)]))
  const project = listed('pagis-desktop', { version: '1.2.3', dependencies: PRODUCTION })
  const treeFile = path.join(bin, 'tree.json')
  fs.writeFileSync(treeFile, JSON.stringify([{ name: 'pagis-desktop', ...project }]))
  fs.writeFileSync(
    path.join(bin, 'pnpm'),
    [
      '#!/bin/sh',
      'if [ "$*" = "list --prod --depth Infinity --json --filter ./desktop" ] && [ -f desktop/scripts/check-package.mjs ]; then',
      `  exec cat '${treeFile}'`,
      'fi',
      'echo "unexpected pnpm $* in $(pwd)" >&2',
      'exit 1',
      '',
    ].join('\n'),
    { mode: 0o755 },
  )
  return { ...process.env, PATH: `${bin}${path.delimiter}${process.env.PATH}` }
}

/**
 * A copy of the repository layout around `scripts/`, whose
 * `docs/update-key.pem` is `updateKey`. check-package.mjs compares the key
 * in a Linux package with that file, so a test runs the script there with
 * a key that it generates, never with the real key.
 */
function repository(updateKey: string): string {
  const root = temporary()
  const scripts = path.join(root, 'desktop', 'scripts')
  fs.mkdirSync(scripts, { recursive: true })
  fs.copyFileSync(path.join(SCRIPTS, 'check-package.mjs'), path.join(scripts, 'check-package.mjs'))
  fs.symlinkSync(path.join(__dirname, '..', 'node_modules'), path.join(root, 'desktop', 'node_modules'))
  fs.mkdirSync(path.join(root, 'docs'))
  fs.writeFileSync(path.join(root, 'docs', 'update-key.pem'), updateKey)
  return scripts
}

/** An Ed25519 public key in SPKI PEM, as `openssl pkey -pubout` writes it,
 *  and its private key in PKCS#8 PEM. */
function updateKeyPair(): { publicPem: string; privatePem: string } {
  const { publicKey, privateKey } = crypto.generateKeyPairSync('ed25519')
  return {
    publicPem: publicKey.export({ type: 'spki', format: 'pem' }) as string,
    privatePem: privateKey.export({ type: 'pkcs8', format: 'pem' }) as string,
  }
}

describe('the Update feed of the macOS ZIP', () => {
  function release(feed: (zip: { name: string; sha512: string; size: number }) => unknown) {
    const root = temporary()
    const zip = path.join(root, 'Pagis-1.2.3-arm64.zip')
    fs.writeFileSync(zip, crypto.randomBytes(4096))
    const bytes = fs.readFileSync(zip)
    const file = {
      name: path.basename(zip),
      sha512: crypto.createHash('sha512').update(bytes).digest('base64'),
      size: bytes.length,
    }
    const feedFile = path.join(root, 'latest-mac.yml')
    // A JSON document is a YAML document too.
    fs.writeFileSync(feedFile, JSON.stringify(feed(file)))
    return [feedFile, '1.2.3', zip]
  }

  const good = (zip: { name: string; sha512: string; size: number }) => ({
    version: '1.2.3',
    files: [{ url: zip.name, sha512: zip.sha512, size: zip.size }],
    path: zip.name,
    sha512: zip.sha512,
    releaseDate: '2026-01-01T00:00:00.000Z',
  })

  it('accepts a feed that names the ZIP with its SHA-512 and size', () => {
    expect(check('check-update-feed.mjs', release(good))).toBeNull()
  })

  it('refuses a feed that does not match the ZIP', () => {
    const cases: [string, (zip: { name: string; sha512: string; size: number }) => unknown][] = [
      ['version', (zip) => ({ ...good(zip), version: '1.2.4' })],
      ['SHA-512', (zip) => ({ ...good(zip), files: [{ url: zip.name, sha512: 'AAAA', size: zip.size }] })],
      ['size', (zip) => ({ ...good(zip), files: [{ url: zip.name, sha512: zip.sha512, size: zip.size + 1 }] })],
      ['Pagis-1.2.3-arm64.zip', (zip) => ({ ...good(zip), files: [{ url: 'Pagis-1.2.3.zip', sha512: zip.sha512, size: zip.size }] })],
      ['no other file', (zip) => ({
        ...good(zip),
        files: [...good(zip).files, { url: 'Pagis-1.2.3-arm64.dmg', sha512: zip.sha512, size: 1 }],
      })],
    ]
    for (const [named, feed] of cases) {
      expect(check('check-update-feed.mjs', release(feed)), named).toContain(named)
    }
  })
})

/** Each Linux architecture has a feed that names its AppImage and its deb.
 *  electron-updater takes the file of its own package from it. */
describe('the Update feed of a Linux architecture', () => {
  type Package = { name: string; sha512: string; size: number }

  function release(feed: (appImage: Package, deb: Package) => unknown) {
    const root = temporary()
    const [appImage, deb] = ['Pagis-1.2.3-x86_64.AppImage', 'Pagis-1.2.3-amd64.deb'].map((name) => {
      const bytes = crypto.randomBytes(4096)
      fs.writeFileSync(path.join(root, name), bytes)
      return { name, sha512: crypto.createHash('sha512').update(bytes).digest('base64'), size: bytes.length }
    })
    const feedFile = path.join(root, 'latest-linux.yml')
    fs.writeFileSync(feedFile, JSON.stringify(feed(appImage, deb)))
    return [feedFile, '1.2.3', path.join(root, appImage.name), path.join(root, deb.name)]
  }

  const entry = (file: Package) => ({ url: file.name, sha512: file.sha512, size: file.size })
  const good = (appImage: Package, deb: Package) => ({
    version: '1.2.3',
    files: [{ ...entry(appImage), blockMapSize: 1234 }, entry(deb)],
    path: appImage.name,
    sha512: appImage.sha512,
    releaseDate: '2026-01-01T00:00:00.000Z',
  })

  it('accepts a feed that names the AppImage and the deb with their SHA-512 and size', () => {
    expect(check('check-update-feed.mjs', release(good))).toBeNull()
  })

  it('refuses a feed that does not name both packages as they are', () => {
    const cases: [string, (appImage: Package, deb: Package) => unknown][] = [
      ['Pagis-1.2.3-amd64.deb', (appImage) => ({ ...good(appImage, appImage), files: [entry(appImage)] })],
      ['another SHA-512 than Pagis-1.2.3-amd64.deb', (appImage, deb) => ({
        ...good(appImage, deb),
        files: [entry(appImage), { ...entry(deb), sha512: appImage.sha512 }],
      })],
      ['the size', (appImage, deb) => ({
        ...good(appImage, deb),
        files: [{ ...entry(appImage), size: appImage.size + 1 }, entry(deb)],
      })],
      ['no other file', (appImage, deb) => ({
        ...good(appImage, deb),
        files: [entry(appImage), entry(deb), { url: 'Pagis-1.2.3-arm64.deb', sha512: deb.sha512, size: deb.size }],
      })],
    ]
    for (const [named, feed] of cases) {
      expect(check('check-update-feed.mjs', release(feed)), named).toContain(named)
    }
  })
})

describe('the inventory of the packaged app', () => {
  async function packagedApp(updateConfig: string | null, packed = PACKED): Promise<string[]> {
    const root = temporary()
    const source = path.join(root, 'source')
    for (const name of ['static/trayTemplate.png', 'static/trayTemplate@2x.png', 'dist/design/tokens.css',
      'dist/design/inter.woff2', 'dist/design/inter.LICENSE', 'dist/design/pagis-mark.svg',
      'dist/design/pagis-mark-dark.svg']) {
      fs.mkdirSync(path.dirname(path.join(source, name)), { recursive: true })
      fs.writeFileSync(path.join(source, name), name)
    }
    writePacked(source, packed)
    const app = path.join(root, 'Pagis.app')
    const resources = path.join(app, 'Contents', 'Resources')
    fs.mkdirSync(resources, { recursive: true })
    await createPackage(source, path.join(resources, 'app.asar'))
    fs.writeFileSync(path.join(resources, 'runtime-lock.json'), '{}')
    writeNotices(resources)
    if (updateConfig !== null) fs.writeFileSync(path.join(resources, 'app-update.yml'), updateConfig)
    const dmg = path.join(root, 'Pagis-1.2.3-arm64.dmg')
    fs.writeFileSync(dmg, 'dmg')
    return [app, dmg]
  }

  /** The unpacked Linux app, as electron-builder writes it beside the
   *  AppImage and the deb. */
  async function linuxApp(updateKey: string | null): Promise<string[]> {
    const root = temporary()
    const source = path.join(root, 'source')
    for (const name of ['static/tray.png', 'dist/design/tokens.css', 'dist/design/inter.woff2',
      'dist/design/inter.LICENSE', 'dist/design/pagis-mark.svg', 'dist/design/pagis-mark-dark.svg']) {
      fs.mkdirSync(path.dirname(path.join(source, name)), { recursive: true })
      fs.writeFileSync(path.join(source, name), name)
    }
    writePacked(source, PACKED)
    const app = path.join(root, 'linux-unpacked')
    const resources = path.join(app, 'resources')
    fs.mkdirSync(resources, { recursive: true })
    await createPackage(source, path.join(resources, 'app.asar'))
    fs.writeFileSync(path.join(resources, 'runtime-lock.json'), '{}')
    fs.writeFileSync(path.join(resources, 'app-update.yml'), 'owner: pagis-co\nrepo: pagis\nprovider: github\n')
    writeNotices(resources)
    if (updateKey !== null) fs.writeFileSync(path.join(resources, 'update-key.pem'), updateKey)
    const packages = ['Pagis-1.2.3-x86_64.AppImage', 'Pagis-1.2.3-amd64.deb'].map((name) => path.join(root, name))
    for (const file of packages) fs.writeFileSync(file, 'package')
    return [app, ...packages]
  }

  /** The notices of the npm packages, and the notices of Electron and
   *  Chromium that electron-builder copies beside them. */
  function writeNotices(resources: string) {
    for (const name of NOTICES) fs.writeFileSync(path.join(resources, name), name)
  }

  const NOTICES = ['THIRD_PARTY_NOTICES', 'LICENSE.electron.txt', 'LICENSES.chromium.html']

  const CONFIG = 'owner: pagis-co\nrepo: pagis\nprovider: github\nupdaterCacheDirName: pagis-desktop-updater\n'

  it('refuses an app with no third-party notices', async () => {
    for (const name of NOTICES) {
      const [app, dmg] = await packagedApp(CONFIG)
      fs.rmSync(path.join(app, 'Contents', 'Resources', name))

      expect(check('check-package.mjs', [app, dmg], SCRIPTS, pnpmEnvironment()), name).toContain(name)
    }
  })

  it('accepts an app with its app-update.yml and its production dependencies', async () => {
    expect(check('check-package.mjs', await packagedApp(CONFIG), SCRIPTS, pnpmEnvironment())).toBeNull()
  })

  /** The Linux package embeds the public Update Key, which checks the
   *  signed checksum list of an Update (ADR-0027). */
  it('accepts a Linux app that holds the Update Key of the repository', async () => {
    const { publicPem } = updateKeyPair()

    expect(check('check-package.mjs', await linuxApp(publicPem), repository(publicPem), pnpmEnvironment())).toBeNull()
  })

  it('refuses a Linux app with no Update Key, or with another key', async () => {
    const scripts = repository(updateKeyPair().publicPem)

    expect(check('check-package.mjs', await linuxApp(null), scripts, pnpmEnvironment())).toContain('update-key.pem')
    expect(check('check-package.mjs', await linuxApp(updateKeyPair().publicPem), scripts, pnpmEnvironment())).toContain(
      'the update-key.pem of the client package is not docs/update-key.pem',
    )
  })

  /** The private half of the Update Key is a secret of the release, and
   *  never a file of the repository or of a package. */
  it('refuses an Update Key that is not an Ed25519 public key in SPKI PEM', async () => {
    const { privatePem } = updateKeyPair()
    const rsa = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 }).publicKey
      .export({ type: 'spki', format: 'pem' }) as string
    for (const key of [privatePem, rsa]) {
      expect(check('check-package.mjs', await linuxApp(key), repository(key), pnpmEnvironment())).toContain(
        'docs/update-key.pem is not an Ed25519 public key in SPKI PEM',
      )
    }
  })

  /** A missing dependency of electron-updater breaks Updates only on the
   *  machine of a person, so the packaging finds it. */
  it('refuses an app archive that leaves out a production dependency', async () => {
    const { 'node_modules/builder-util-runtime': _left, ...packed } = PACKED

    expect(check('check-package.mjs', await packagedApp(CONFIG, packed), SCRIPTS, pnpmEnvironment())).toContain(
      'the client app archive has no builder-util-runtime@9.7.0',
    )
  })

  /** Each package must be in the archive at the version that pnpm
   *  resolved for it, also when two packages need two versions of it. */
  it('refuses an app archive that holds another version of a production dependency', async () => {
    const { 'node_modules/electron-updater/node_modules/semver': _left, ...packed } = PACKED

    expect(check('check-package.mjs', await packagedApp(CONFIG, packed), SCRIPTS, pnpmEnvironment())).toContain(
      'the client app archive has no semver@7.7.4',
    )
  })

  /** electron-updater reads the name of its download cache from it. */
  it('refuses an app with no app-update.yml', async () => {
    expect(check('check-package.mjs', await packagedApp(null), SCRIPTS, pnpmEnvironment())).toContain(
      'the client package has no app-update.yml',
    )
  })
})
