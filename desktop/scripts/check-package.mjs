import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as path from 'node:path'
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const require = createRequire(import.meta.url)
const asar = require('@electron/asar')

// The app is Pagis.app on macOS and the unpacked directory on Linux. The
// packages are what a person downloads: the DMG and the ZIP, or the
// AppImage and the deb.
const app = process.argv[2]
const packages = process.argv.slice(3)
if (!app || packages.length === 0) throw new Error('usage: node scripts/check-package.mjs <app> <package>...')

const mac = app.endsWith('.app')
const resources = mac ? path.join(app, 'Contents', 'Resources') : path.join(app, 'resources')
// On Linux the public Update Key checks the signed checksum list of an
// Update (ADR-0027). electron-updater reads the name of its download cache
// from app-update.yml. The Client App sets the feed itself at each check.
const required = ['app.asar', 'runtime-lock.json', 'app-update.yml', ...(mac ? [] : ['update-key.pem'])]
for (const name of required) {
  if (!fs.statSync(path.join(resources, name), { throwIfNoEntry: false })?.isFile()) {
    throw new Error(`the client package has no ${name}`)
  }
}
// The package holds the public key of the repository. The private half is
// a secret of the release, so the repository file must be an Ed25519
// public key in SPKI PEM, as `openssl pkey -pubout` writes it.
if (!mac) {
  const repositoryFile = fileURLToPath(new URL('../../docs/update-key.pem', import.meta.url))
  if (!fs.existsSync(repositoryFile)) throw new Error('the repository has no docs/update-key.pem')
  const repositoryKey = fs.readFileSync(repositoryFile, 'utf8')
  let publicKey
  try {
    publicKey = crypto.createPublicKey(repositoryKey)
  } catch {
    publicKey = null
  }
  if (publicKey?.asymmetricKeyType !== 'ed25519' || publicKey.export({ type: 'spki', format: 'pem' }) !== repositoryKey) {
    throw new Error('docs/update-key.pem is not an Ed25519 public key in SPKI PEM')
  }
  if (fs.readFileSync(path.join(resources, 'update-key.pem'), 'utf8') !== repositoryKey) {
    throw new Error('the update-key.pem of the client package is not docs/update-key.pem')
  }
}

const forbiddenNames = new Set(['pagis', 'gog', 'secrets.enc'])
const forbiddenSuffixes = ['.db', '.sqlite', '.tar', '.tar.gz', '.layer']
for (const file of walk(resources)) {
  const name = path.basename(file)
  if (forbiddenNames.has(name) || forbiddenSuffixes.some((suffix) => name.endsWith(suffix))) {
    throw new Error(`the client package contains server or Workspace data: ${path.relative(resources, file)}`)
  }
}
// The tray draws these files. macOS loads the @2x file beside the
// template image for a Retina screen.
const archived = new Set(asar.listPackage(path.join(resources, 'app.asar')))
const trayIcons = mac ? ['trayTemplate.png', 'trayTemplate@2x.png'] : ['tray.png']
for (const name of trayIcons) {
  if (!archived.has(`/static/${name}`)) throw new Error(`the client app archive has no static/${name}`)
}
// The setup and status pages load the design system from the package,
// with no network.
for (const name of ['tokens.css', 'inter.woff2', 'pagis-mark.svg', 'pagis-mark-dark.svg']) {
  if (!archived.has(`/dist/design/${name}`)) throw new Error(`the client app archive has no dist/design/${name}`)
}
for (const entry of archived) {
  const name = path.posix.basename(entry)
  if (forbiddenNames.has(name) || forbiddenSuffixes.some((suffix) => name.endsWith(suffix))) {
    throw new Error(`the client app archive contains server or Workspace data: ${entry}`)
  }
}

// A package that holds a server binary is at or above these sizes, so a
// client-only package and app stay below them.
const packageCeilingBytes = 174_423_479
const appCeilingKiB = 418_672
const sizes = packages.map((file) => {
  const bytes = fs.statSync(file).size
  if (bytes >= packageCeilingBytes) {
    throw new Error(`the client package ${path.basename(file)} is ${bytes} bytes, at or above the ${packageCeilingBytes}-byte ceiling`)
  }
  return `${path.basename(file)} ${bytes} bytes`
})
const appKiB = Number.parseInt(execFileSync('du', ['-sk', app], { encoding: 'utf8' }), 10)
if (appKiB >= appCeilingKiB) {
  throw new Error(`the client app uses ${appKiB} KiB, at or above the ${appCeilingKiB}-KiB ceiling`)
}

console.log(`pagis package inventory: client-only Resources verified; ${sizes.join('; ')}; app ${appKiB} KiB`)

function * walk(root) {
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const file = path.join(root, entry.name)
    if (entry.isDirectory()) yield * walk(file)
    else if (entry.isFile()) yield file
    else throw new Error(`the client package contains an unsupported Resources entry: ${entry.name}`)
  }
}
