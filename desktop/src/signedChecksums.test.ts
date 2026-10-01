// The trust of an Update on Linux (ADR-0027): the release key signs the
// checksum list of a release, and a download must have the SHA-256 on its
// line. Each test signs with a key that it generates, never a real key.

import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import * as openpgp from 'openpgp'
import { afterEach, beforeAll, describe, expect, it } from 'vitest'

import { checkSignedChecksum } from './signedChecksums'

const RELEASE = 'https://github.com/pagis-co/pagis/releases/download/v1.2.0'
const LIST = 'Pagis-1.2.0-linux.SHA256SUMS'
const APPIMAGE = 'Pagis-1.2.0-x86_64.AppImage'

interface KeyPair {
  privateKey: string
  publicKey: string
}

let releaseKey: KeyPair
let otherKey: KeyPair

beforeAll(async () => {
  releaseKey = await openpgp.generateKey({ userIDs: [{ name: 'Test release key' }], format: 'armored' })
  otherKey = await openpgp.generateKey({ userIDs: [{ name: 'Another key' }], format: 'armored' })
})

const roots: string[] = []
afterEach(() => {
  while (roots.length > 0) fs.rmSync(roots.pop()!, { recursive: true, force: true })
})

/** A downloaded Update in the cache of electron-updater, under its release name. */
function download(name: string, bytes: Buffer): string {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-update-'))
  roots.push(root)
  const file = path.join(root, name)
  fs.writeFileSync(file, bytes)
  return file
}

function sha256(bytes: Buffer): string {
  return crypto.createHash('sha256').update(bytes).digest('hex')
}

/** A detached signature, as `gpg --armor --detach-sign` makes one. */
async function sign(list: string, key: KeyPair): Promise<string> {
  const signingKeys = await openpgp.readPrivateKey({ armoredKey: key.privateKey })
  const message = await openpgp.createMessage({ binary: Buffer.from(list) })
  return (await openpgp.sign({ message, signingKeys, detached: true, format: 'armored' })) as string
}

/** The release files on GitHub. Each other address answers 404. */
function github(files: Record<string, string>): { request: typeof fetch; asked: string[] } {
  const asked: string[] = []
  const request = (async (url: string | URL) => {
    asked.push(String(url))
    const body = files[String(url)]
    return body === undefined ? new Response('Not Found', { status: 404 }) : new Response(body)
  }) as typeof fetch
  return { request, asked }
}

describe('the signed checksum list of an Update', () => {
  it('accepts a download whose SHA-256 is on its line of a list that the release key signed', async () => {
    const bytes = crypto.randomBytes(4096)
    const file = download(APPIMAGE, bytes)
    const list = [
      `${sha256(bytes)}  ${APPIMAGE}`,
      `${sha256(Buffer.from('deb'))}  Pagis-1.2.0-amd64.deb`,
      '',
    ].join('\n')
    const { request, asked } = github({
      [`${RELEASE}/${LIST}`]: list,
      [`${RELEASE}/${LIST}.asc`]: await sign(list, releaseKey),
    })

    await expect(checkSignedChecksum(file, '1.2.0', releaseKey.publicKey, request)).resolves.toBeUndefined()
    expect(asked.sort()).toEqual([`${RELEASE}/${LIST}`, `${RELEASE}/${LIST}.asc`])
  })

  it('refuses a list that another key signed', async () => {
    const bytes = crypto.randomBytes(64)
    const list = `${sha256(bytes)}  ${APPIMAGE}\n`
    const { request } = github({
      [`${RELEASE}/${LIST}`]: list,
      [`${RELEASE}/${LIST}.asc`]: await sign(list, otherKey),
    })

    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', releaseKey.publicKey, request))
      .rejects.toThrow(`${LIST} does not verify with the Pagis release key`)
  })

  it('refuses a list that changed after the release key signed it', async () => {
    const bytes = crypto.randomBytes(64)
    const signed = `${sha256(Buffer.from('the real package'))}  ${APPIMAGE}\n`
    const { request } = github({
      [`${RELEASE}/${LIST}`]: `${sha256(bytes)}  ${APPIMAGE}\n`,
      [`${RELEASE}/${LIST}.asc`]: await sign(signed, releaseKey),
    })

    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', releaseKey.publicKey, request))
      .rejects.toThrow(`${LIST} does not verify with the Pagis release key`)
  })

  it('refuses a signature file that holds no OpenPGP signature', async () => {
    const bytes = crypto.randomBytes(64)
    const { request } = github({
      [`${RELEASE}/${LIST}`]: `${sha256(bytes)}  ${APPIMAGE}\n`,
      [`${RELEASE}/${LIST}.asc`]: 'not a signature',
    })

    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', releaseKey.publicKey, request))
      .rejects.toThrow(`${LIST} does not verify with the Pagis release key`)
  })

  it('refuses a download whose SHA-256 is not the one on its line', async () => {
    const list = `${sha256(Buffer.from('the real package'))}  ${APPIMAGE}\n`
    const { request } = github({
      [`${RELEASE}/${LIST}`]: list,
      [`${RELEASE}/${LIST}.asc`]: await sign(list, releaseKey),
    })

    await expect(checkSignedChecksum(download(APPIMAGE, crypto.randomBytes(64)), '1.2.0', releaseKey.publicKey, request))
      .rejects.toThrow(`the SHA-256 of the downloaded ${APPIMAGE} is not the one in ${LIST}`)
  })

  it('refuses a download that has no line, or two lines, in the list', async () => {
    const bytes = crypto.randomBytes(64)
    for (const list of [
      `${sha256(bytes)}  Pagis-1.2.0-arm64.AppImage\n`,
      `${sha256(bytes)}  ${APPIMAGE}\n${sha256(bytes)} *${APPIMAGE}\n`,
    ]) {
      const { request } = github({
        [`${RELEASE}/${LIST}`]: list,
        [`${RELEASE}/${LIST}.asc`]: await sign(list, releaseKey),
      })

      await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', releaseKey.publicKey, request))
        .rejects.toThrow(`${LIST} does not have one line for ${APPIMAGE}`)
    }
  })

  it('refuses the Update when the release has no list or no signature', async () => {
    const bytes = crypto.randomBytes(64)
    const list = `${sha256(bytes)}  ${APPIMAGE}\n`

    const noSignature = github({ [`${RELEASE}/${LIST}`]: list })
    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', releaseKey.publicKey, noSignature.request))
      .rejects.toThrow(`the download of ${LIST}.asc answered HTTP 404`)

    const offline = (async () => {
      throw new TypeError('fetch failed')
    }) as unknown as typeof fetch
    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', releaseKey.publicKey, offline))
      .rejects.toThrow(`the download of ${LIST} failed: fetch failed`)
  })

  /** The key that the Linux package embeds, in the form openpgp.js reads. */
  it('reads the public release key of the repository', async () => {
    const armoredKey = fs.readFileSync(path.join(__dirname, '..', '..', 'docs', 'release-key.asc'), 'utf8')

    const key = await openpgp.readKey({ armoredKey })

    expect(key.isPrivate()).toBe(false)
  })
})
