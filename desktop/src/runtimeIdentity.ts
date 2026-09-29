/**
 * The proof that the server on this machine is the one this client
 * started (ADR-0025).
 *
 * It is keyed by the Client Credential of the installation, a
 * file the OS gives to its owner alone, so only a process that holds
 * that file can answer and only this client can check the answer. It
 * belongs to the install-and-supervise path: a server the client did not
 * start holds no Client Credential, answers nothing here, and is trusted
 * through TLS and the person's sign-in instead.
 */

import * as crypto from 'node:crypto'

const HASH = /^[0-9a-f]{64}$/

export interface RuntimeIdentity {
  release: string
  workspaceId: string
  computerImage: string
}

export async function probeRuntimeIdentity(
  origin: string,
  port: number,
  credential: string,
  expectedRelease: string | null,
  expectedImage: string | null,
  fetcher: typeof fetch = fetch,
  challenge = crypto.randomBytes(32).toString('hex'),
): Promise<RuntimeIdentity | null> {
  if (!HASH.test(challenge)) throw new Error('runtime identity challenge must be 32 random bytes')
  try {
    const url = new URL(`/api/v1/runtime/identity?challenge=${challenge}`, origin)
    const response = await fetcher(url, {
      redirect: 'error',
      signal: AbortSignal.timeout(1000),
    })
    if (!response.ok || response.redirected) return null
    const value = await response.json() as unknown
    if (value === null || typeof value !== 'object' || Array.isArray(value)) return null
    const body = value as Record<string, unknown>
    const fields = ['status', 'release', 'workspace_id', 'port', 'computer_image', 'proof'].sort()
    const actual = Object.keys(body).sort()
    if (actual.length !== fields.length || actual.some((field, index) => field !== fields[index])) return null
    if (
      body.status !== 'ok' ||
      body.port !== port ||
      (expectedRelease !== null && body.release !== expectedRelease) ||
      (expectedImage !== null && body.computer_image !== expectedImage) ||
      typeof body.release !== 'string' ||
      body.release.length === 0 ||
      typeof body.computer_image !== 'string' ||
      typeof body.workspace_id !== 'string' ||
      body.workspace_id.length === 0 ||
      body.workspace_id.length > 128 ||
      typeof body.proof !== 'string' ||
      !HASH.test(body.proof)
    ) return null
    const message = [
      'pagis-runtime-identity-v1',
      String(port),
      body.release,
      body.workspace_id,
      body.computer_image,
      challenge,
    ].join('\0')
    const expected = crypto.createHmac('sha256', credential).update(message).digest()
    const found = Buffer.from(body.proof, 'hex')
    if (found.length !== expected.length || !crypto.timingSafeEqual(found, expected)) return null
    return {
      release: body.release,
      workspaceId: body.workspace_id,
      computerImage: body.computer_image,
    }
  } catch {
    return null
  }
}

export async function waitForRuntimeIdentity(
  origin: string,
  port: number,
  credential: () => string | null,
  release: string,
  image: string,
  options: { timeoutMs: number; intervalMs?: number; cancelled?: () => boolean },
): Promise<RuntimeIdentity | null> {
  const deadline = Date.now() + options.timeoutMs
  while (Date.now() < deadline) {
    if (options.cancelled?.()) return null
    const currentCredential = credential()
    if (currentCredential) {
      const identity = await probeRuntimeIdentity(origin, port, currentCredential, release, image)
      if (identity) return identity
    }
    await new Promise((resolve) => setTimeout(resolve, options.intervalMs ?? 100))
  }
  return null
}
