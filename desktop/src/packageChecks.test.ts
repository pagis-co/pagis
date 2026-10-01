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
const RELEASE_KEY = path.join(__dirname, '..', '..', 'docs', 'release-key.asc')

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
function check(script: string, args: string[]): string | null {
  try {
    execFileSync(process.execPath, [path.join(SCRIPTS, script), ...args], { stdio: 'pipe' })
    return null
  } catch (error) {
    return String((error as { stderr: Buffer }).stderr)
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
  async function packagedApp(updateConfig: string | null): Promise<string[]> {
    const root = temporary()
    const source = path.join(root, 'source')
    for (const name of ['static/trayTemplate.png', 'static/trayTemplate@2x.png', 'dist/design/tokens.css',
      'dist/design/inter.woff2', 'dist/design/pagis-mark.svg', 'dist/design/pagis-mark-dark.svg']) {
      fs.mkdirSync(path.dirname(path.join(source, name)), { recursive: true })
      fs.writeFileSync(path.join(source, name), name)
    }
    const app = path.join(root, 'Pagis.app')
    const resources = path.join(app, 'Contents', 'Resources')
    fs.mkdirSync(resources, { recursive: true })
    await createPackage(source, path.join(resources, 'app.asar'))
    fs.writeFileSync(path.join(resources, 'runtime-lock.json'), '{}')
    if (updateConfig !== null) fs.writeFileSync(path.join(resources, 'app-update.yml'), updateConfig)
    const dmg = path.join(root, 'Pagis-1.2.3-arm64.dmg')
    fs.writeFileSync(dmg, 'dmg')
    return [app, dmg]
  }

  /** The unpacked Linux app, as electron-builder writes it beside the
   *  AppImage and the deb. */
  async function linuxApp(releaseKey: Buffer | null): Promise<string[]> {
    const root = temporary()
    const source = path.join(root, 'source')
    for (const name of ['static/tray.png', 'dist/design/tokens.css', 'dist/design/inter.woff2',
      'dist/design/pagis-mark.svg', 'dist/design/pagis-mark-dark.svg']) {
      fs.mkdirSync(path.dirname(path.join(source, name)), { recursive: true })
      fs.writeFileSync(path.join(source, name), name)
    }
    const app = path.join(root, 'linux-unpacked')
    const resources = path.join(app, 'resources')
    fs.mkdirSync(resources, { recursive: true })
    await createPackage(source, path.join(resources, 'app.asar'))
    fs.writeFileSync(path.join(resources, 'runtime-lock.json'), '{}')
    fs.writeFileSync(path.join(resources, 'app-update.yml'), 'owner: pagis-co\nrepo: pagis\nprovider: github\n')
    if (releaseKey !== null) fs.writeFileSync(path.join(resources, 'release-key.asc'), releaseKey)
    const packages = ['Pagis-1.2.3-x86_64.AppImage', 'Pagis-1.2.3-amd64.deb'].map((name) => path.join(root, name))
    for (const file of packages) fs.writeFileSync(file, 'package')
    return [app, ...packages]
  }

  it('accepts an app whose app-update.yml names the GitHub releases of pagis-co/pagis', async () => {
    const config = 'owner: pagis-co\nrepo: pagis\nprovider: github\nupdaterCacheDirName: pagis-desktop-updater\n'

    expect(check('check-package.mjs', await packagedApp(config))).toBeNull()
  })

  /** The Linux package embeds the public release key, which checks the
   *  signed checksum list of an Update (ADR-0027). */
  it('accepts a Linux app that holds the release key of the repository', async () => {
    expect(check('check-package.mjs', await linuxApp(fs.readFileSync(RELEASE_KEY)))).toBeNull()
  })

  it('refuses a Linux app with no release key, or with another key', async () => {
    expect(check('check-package.mjs', await linuxApp(null))).toContain('release-key.asc')
    expect(check('check-package.mjs', await linuxApp(Buffer.from('another key')))).toContain(
      'the release-key.asc of the client package is not docs/release-key.asc',
    )
  })

  it('refuses an app with no app-update.yml, or one that names another feed', async () => {
    expect(check('check-package.mjs', await packagedApp(null))).toContain('app-update.yml')
    for (const config of [
      'owner: someone\nrepo: pagis\nprovider: github\n',
      'owner: pagis-co\nrepo: other\nprovider: github\n',
      'provider: generic\nurl: https://example.test/\n',
    ]) {
      expect(check('check-package.mjs', await packagedApp(config)), config).toContain('app-update.yml')
    }
  })
})
