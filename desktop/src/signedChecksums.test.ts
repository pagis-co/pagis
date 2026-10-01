// The trust of an Update on Linux (ADR-0027): the Update Key signs the
// checksum list of a release, and a download must have the SHA-256 on its
// line. Each test signs with a key that it generates, never a real key.

import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { checkSignedChecksum } from './signedChecksums'

const RELEASE = 'https://github.com/pagis-co/pagis/releases/download/v1.2.0'
const LIST = 'Pagis-1.2.0-linux.SHA256SUMS'
const APPIMAGE = 'Pagis-1.2.0-x86_64.AppImage'

/** A key pair as the release holds it: the private key signs, and the
 *  package embeds the public key as SPKI PEM. */
function keyPair(type: 'ed25519' | 'ed448' = 'ed25519') {
  const { privateKey, publicKey } = type === 'ed25519'
    ? crypto.generateKeyPairSync('ed25519')
    : crypto.generateKeyPairSync('ed448')
  return { privateKey, publicPem: publicKey.export({ type: 'spki', format: 'pem' }) as string }
}

const updateKey = keyPair()
const otherKey = keyPair()

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

/** The raw 64-byte Ed25519 signature, as `openssl pkeyutl -sign -rawin` writes it. */
function sign(list: string, key: crypto.KeyObject = updateKey.privateKey): Buffer {
  return crypto.sign(null, Buffer.from(list), key)
}

/** The release files on GitHub. Each other address answers 404. */
function github(files: Record<string, string | Buffer>): { request: typeof fetch; asked: string[] } {
  const asked: string[] = []
  const request = (async (url: string | URL) => {
    asked.push(String(url))
    const body = files[String(url)]
    return body === undefined ? new Response('Not Found', { status: 404 }) : new Response(new Uint8Array(Buffer.from(body)))
  }) as typeof fetch
  return { request, asked }
}

function release(list: string, signature: Buffer) {
  return github({ [`${RELEASE}/${LIST}`]: list, [`${RELEASE}/${LIST}.sig`]: signature })
}

describe('the signed checksum list of an Update', () => {
  it('accepts a download whose SHA-256 is on its line of a list that the Update Key signed', async () => {
    const bytes = crypto.randomBytes(4096)
    const list = [
      `${sha256(bytes)}  ${APPIMAGE}`,
      `${sha256(Buffer.from('deb'))}  Pagis-1.2.0-amd64.deb`,
      '',
    ].join('\n')
    const { request, asked } = release(list, sign(list))

    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', updateKey.publicPem, request))
      .resolves.toBeUndefined()
    expect(asked.sort()).toEqual([`${RELEASE}/${LIST}`, `${RELEASE}/${LIST}.sig`])
  })

  it('refuses a list that another key signed', async () => {
    const bytes = crypto.randomBytes(64)
    const list = `${sha256(bytes)}  ${APPIMAGE}\n`
    const { request } = release(list, sign(list, otherKey.privateKey))

    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', updateKey.publicPem, request))
      .rejects.toThrow(`${LIST} does not verify with the Pagis Update Key`)
  })

  it('refuses a list that changed after the Update Key signed it', async () => {
    const bytes = crypto.randomBytes(64)
    const signed = `${sha256(Buffer.from('the real package'))}  ${APPIMAGE}\n`
    const { request } = release(`${sha256(bytes)}  ${APPIMAGE}\n`, sign(signed))

    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', updateKey.publicPem, request))
      .rejects.toThrow(`${LIST} does not verify with the Pagis Update Key`)
  })

  it('refuses a signature file that holds no Ed25519 signature', async () => {
    const bytes = crypto.randomBytes(64)
    const { request } = release(`${sha256(bytes)}  ${APPIMAGE}\n`, Buffer.from('not a signature'))

    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', updateKey.publicPem, request))
      .rejects.toThrow(`${LIST} does not verify with the Pagis Update Key`)
  })

  it('refuses an Update Key that is not an Ed25519 public key', async () => {
    const bytes = crypto.randomBytes(64)
    const list = `${sha256(bytes)}  ${APPIMAGE}\n`
    const ed448 = keyPair('ed448')
    const { request } = release(list, crypto.sign(null, Buffer.from(list), ed448.privateKey))

    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', ed448.publicPem, request))
      .rejects.toThrow('the Pagis Update Key is not an Ed25519 public key')
  })

  it('refuses a download whose SHA-256 is not the one on its line', async () => {
    const list = `${sha256(Buffer.from('the real package'))}  ${APPIMAGE}\n`
    const { request } = release(list, sign(list))

    await expect(checkSignedChecksum(download(APPIMAGE, crypto.randomBytes(64)), '1.2.0', updateKey.publicPem, request))
      .rejects.toThrow(`the SHA-256 of the downloaded ${APPIMAGE} is not the one in ${LIST}`)
  })

  it('refuses a download that has no line, or two lines, in the list', async () => {
    const bytes = crypto.randomBytes(64)
    for (const list of [
      `${sha256(bytes)}  Pagis-1.2.0-arm64.AppImage\n`,
      `${sha256(bytes)}  ${APPIMAGE}\n${sha256(bytes)} *${APPIMAGE}\n`,
    ]) {
      const { request } = release(list, sign(list))

      await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', updateKey.publicPem, request))
        .rejects.toThrow(`${LIST} does not have one line for ${APPIMAGE}`)
    }
  })

  it('refuses the Update when the release has no list or no signature', async () => {
    const bytes = crypto.randomBytes(64)
    const list = `${sha256(bytes)}  ${APPIMAGE}\n`

    const noSignature = github({ [`${RELEASE}/${LIST}`]: list })
    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', updateKey.publicPem, noSignature.request))
      .rejects.toThrow(`the download of ${LIST}.sig answered HTTP 404`)

    const offline = (async () => {
      throw new TypeError('fetch failed')
    }) as unknown as typeof fetch
    await expect(checkSignedChecksum(download(APPIMAGE, bytes), '1.2.0', updateKey.publicPem, offline))
      .rejects.toThrow(`the download of ${LIST} failed: fetch failed`)
  })
})
