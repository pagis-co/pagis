import * as crypto from 'node:crypto'

import { describe, expect, it } from 'vitest'

import { probeRuntimeIdentity } from './runtimeIdentity'

const IMAGE = `ghcr.io/pagis-co/pagis-computer@sha256:${'b'.repeat(64)}`
const CHALLENGE = 'a'.repeat(64)
const RUST_VECTOR = 'b7ddc2ba3d3245faf25d7c71cb11317fa35aaa165b040f9288afabc717012853'

/** The key is the Client Credential; this value is the Rust test vector. */
const CREDENTIAL = 'token-123'
/** The origin of a server this client started (ADR-0025). */
const LOCAL = 'http://127.0.0.1:4400/'

function proof(port: number, release = '0.1.0', image = IMAGE): string {
  const message = ['pagis-runtime-identity-v1', String(port), release, 'workspace-1', image, CHALLENGE].join('\0')
  return crypto.createHmac('sha256', CREDENTIAL).update(message).digest('hex')
}

function response(port: number, overrides: Record<string, unknown> = {}): Response {
  return new Response(JSON.stringify({
    status: 'ok',
    release: '0.1.0',
    workspace_id: 'workspace-1',
    port,
    computer_image: IMAGE,
    proof: proof(port),
    ...overrides,
  }), { status: 200, headers: { 'content-type': 'application/json' } })
}

describe('authenticated runtime identity', () => {
  it('accepts the same Client Credential and exact locked tuple', async () => {
    const identity = await probeRuntimeIdentity(
      LOCAL,
      4400,
      CREDENTIAL,
      '0.1.0',
      IMAGE,
      async () => response(4400, { workspace_id: '01HQTESTWORKSPACE', proof: RUST_VECTOR }),
      CHALLENGE,
    )
    expect(identity).toEqual({ release: '0.1.0', workspaceId: '01HQTESTWORKSPACE', computerImage: IMAGE })
  })

  it('rejects a version-only health spoof and a wrong proof', async () => {
    expect(await probeRuntimeIdentity(LOCAL, 4400, CREDENTIAL, '0.1.0', IMAGE, async () => new Response(JSON.stringify({ status: 'ok', version: '0.1.0' })), CHALLENGE)).toBeNull()
    expect(await probeRuntimeIdentity(LOCAL, 4400, CREDENTIAL, '0.1.0', IMAGE, async () => response(4400, { proof: '0'.repeat(64) }), CHALLENGE)).toBeNull()
  })

  it('rejects a proof relayed from another port or compiled tuple', async () => {
    expect(await probeRuntimeIdentity(LOCAL, 4400, CREDENTIAL, '0.1.0', IMAGE, async () => response(4401), CHALLENGE)).toBeNull()
    expect(await probeRuntimeIdentity(LOCAL, 4400, CREDENTIAL, '0.1.0', IMAGE, async () => response(4400, { computer_image: `ghcr.io/pagis-co/pagis-computer@sha256:${'c'.repeat(64)}` }), CHALLENGE)).toBeNull()
  })
})
