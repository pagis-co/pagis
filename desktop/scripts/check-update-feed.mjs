import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as path from 'node:path'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const yaml = require('js-yaml')

// A feed names the files of an Update for electron-updater (ADR-0027):
// latest-mac.yml the ZIP, and latest-linux.yml and latest-linux-arm64.yml
// the AppImage and the deb of their architecture. A feed is not a trust
// root: Squirrel.Mac checks the code signature of the new app, and on Linux
// the Client App checks the signed checksum list. But electron-updater
// refuses a download whose SHA-512 or size is not the one of the feed, so
// the release checks each feed against the exact files before it publishes
// them.
const [feedFile, version, ...files] = process.argv.slice(2)
if (!feedFile || !version || files.length === 0) {
  throw new Error('usage: node scripts/check-update-feed.mjs <feed> <version> <file>...')
}

const feed = yaml.load(fs.readFileSync(feedFile, 'utf8'))
const feedName = path.basename(feedFile)
const names = files.map((file) => path.basename(file))

if (feed?.version !== version) {
  throw new Error(`${feedName} names version ${feed?.version}, and the release is ${version}`)
}
// The release staples the DMG after electron-builder, so latest-mac.yml
// must not name it. Squirrel.Mac installs the ZIP.
if (!Array.isArray(feed.files) || feed.files.length !== files.length) {
  throw new Error(`${feedName} must name ${names.join(' and ')}, and no other file`)
}
for (const file of files) {
  const name = path.basename(file)
  const entry = feed.files.find((each) => each?.url === name)
  if (!entry) throw new Error(`${feedName} does not name ${name}`)
  const hash = crypto.createHash('sha512')
  for await (const chunk of fs.createReadStream(file)) hash.update(chunk)
  const size = fs.statSync(file).size
  if (entry.sha512 !== hash.digest('base64')) throw new Error(`${feedName} holds another SHA-512 than ${name}`)
  if (entry.size !== size) throw new Error(`${feedName} gives ${name} the size ${entry.size}, not ${size}`)
}

console.log(`pagis update feed: ${feedName} names ${names.join(' and ')}, version ${version}`)
