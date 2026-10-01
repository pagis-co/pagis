import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as path from 'node:path'

/** The GitHub releases of Pagis. Each Linux release holds its checksum
 *  list and the signature of the list. */
const RELEASES = 'https://github.com/pagis-co/pagis/releases/download'
/** How long the client waits for one file of the release. */
const TIMEOUT_MS = 30_000

/**
 * The trust of an Update on Linux (ADR-0027). The Update Key, an Ed25519
 * key, signs `Pagis-<release>-linux.SHA256SUMS`, the SHA-256 of the four
 * Linux packages, into `Pagis-<release>-linux.SHA256SUMS.sig`. The Update
 * in `file` passes when that signature verifies with `updateKey`, the SPKI
 * PEM public key that the package embeds, and the SHA-256 of the file is
 * the value on the line of its name. The file has the release name of the
 * package, as electron-updater downloads it.
 *
 * It throws an error that gives the reason when the Update does not pass.
 */
export async function checkSignedChecksum(
  file: string,
  release: string,
  updateKey: string,
  request: typeof fetch = fetch,
): Promise<void> {
  const key = crypto.createPublicKey(updateKey)
  if (key.asymmetricKeyType !== 'ed25519') throw new Error('the Pagis Update Key is not an Ed25519 public key')
  const listName = `Pagis-${release}-linux.SHA256SUMS`
  const [list, signature] = await Promise.all([
    download(`${RELEASES}/v${release}/${listName}`, listName, request),
    download(`${RELEASES}/v${release}/${listName}.sig`, `${listName}.sig`, request),
  ])
  if (!crypto.verify(null, list, key, signature)) {
    throw new Error(`${listName} does not verify with the Pagis Update Key`)
  }
  const name = path.basename(file)
  const listed = listedSha256(list.toString('utf8'), name)
  if (listed === null) throw new Error(`${listName} does not have one line for ${name}`)
  if ((await sha256(file)) !== listed) {
    throw new Error(`the SHA-256 of the downloaded ${name} is not the one in ${listName}`)
  }
}

async function download(url: string, name: string, request: typeof fetch): Promise<Buffer> {
  let response: Response
  try {
    response = await request(url, { signal: AbortSignal.timeout(TIMEOUT_MS) })
  } catch (error) {
    throw new Error(`the download of ${name} failed: ${error instanceof Error ? error.message : String(error)}`)
  }
  if (!response.ok) throw new Error(`the download of ${name} answered HTTP ${response.status}`)
  return Buffer.from(await response.arrayBuffer())
}

/** The SHA-256 on the one line of `name` in a list that `sha256sum` wrote,
 *  or null when the list does not have exactly one line for it. */
function listedSha256(list: string, name: string): string | null {
  const hashes = list
    .split('\n')
    .map((line) => /^([0-9a-f]{64}) [ *](.+)$/.exec(line))
    .filter((match) => match?.[2] === name)
    .map((match) => match![1])
  return hashes.length === 1 ? hashes[0] : null
}

async function sha256(file: string): Promise<string> {
  const hash = crypto.createHash('sha256')
  for await (const chunk of fs.createReadStream(file)) hash.update(chunk as Buffer)
  return hash.digest('hex')
}
