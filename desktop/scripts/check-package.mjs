import * as fs from 'node:fs'
import * as path from 'node:path'
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const asar = require('@electron/asar')
const yaml = require('js-yaml')

// The app is Pagis.app on macOS and the unpacked directory on Linux. The
// packages are what a person downloads: the DMG and the ZIP, or the
// AppImage and the deb.
const app = process.argv[2]
const packages = process.argv.slice(3)
if (!app || packages.length === 0) throw new Error('usage: node scripts/check-package.mjs <app> <package>...')

const resources = app.endsWith('.app') ? path.join(app, 'Contents', 'Resources') : path.join(app, 'resources')
const required = ['app.asar', 'runtime-lock.json', 'app-update.yml']
for (const name of required) {
  if (!fs.statSync(path.join(resources, name), { throwIfNoEntry: false })?.isFile()) {
    throw new Error(`the client package has no ${name}`)
  }
}
// electron-updater reads the feed that app-update.yml names (ADR-0027).
const feed = yaml.load(fs.readFileSync(path.join(resources, 'app-update.yml'), 'utf8'))
if (feed?.provider !== 'github' || feed.owner !== 'pagis-co' || feed.repo !== 'pagis') {
  throw new Error('the app-update.yml of the client package does not name the GitHub releases of pagis-co/pagis')
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
const trayIcons = app.endsWith('.app') ? ['trayTemplate.png', 'trayTemplate@2x.png'] : ['tray.png']
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
