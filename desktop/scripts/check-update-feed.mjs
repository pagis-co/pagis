import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as path from 'node:path'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const yaml = require('js-yaml')

// latest-mac.yml names the ZIP of an Update for electron-updater
// (ADR-0027). The feed is not a trust root: Squirrel.Mac checks the code
// signature of the new app. But electron-updater refuses a download whose
// SHA-512 or size is not the one of the feed, so the release checks the
// feed against the exact ZIP before it publishes either.
const [feedFile, zip, version] = process.argv.slice(2)
if (!feedFile || !zip || !version) {
  throw new Error('usage: node scripts/check-update-feed.mjs <latest-mac.yml> <zip> <version>')
}

const feed = yaml.load(fs.readFileSync(feedFile, 'utf8'))
const name = path.basename(zip)
const hash = crypto.createHash('sha512')
for await (const chunk of fs.createReadStream(zip)) hash.update(chunk)
const sha512 = hash.digest('base64')
const size = fs.statSync(zip).size

if (feed?.version !== version) {
  throw new Error(`${path.basename(feedFile)} names version ${feed?.version}, and the release is ${version}`)
}
// The release staples the DMG after electron-builder, so the feed must not
// name it. Squirrel.Mac installs the ZIP.
if (!Array.isArray(feed.files) || feed.files.length !== 1) {
  throw new Error(`${path.basename(feedFile)} must name one file, the ZIP ${name}`)
}
const [file] = feed.files
if (file.url !== name) throw new Error(`${path.basename(feedFile)} names ${file.url}, not ${name}`)
if (file.sha512 !== sha512) throw new Error(`${path.basename(feedFile)} holds another SHA-512 than ${name}`)
if (file.size !== size) throw new Error(`${path.basename(feedFile)} gives ${name} the size ${file.size}, not ${size}`)

console.log(`pagis update feed: ${path.basename(feedFile)} names ${name}, ${size} bytes, version ${version}`)
