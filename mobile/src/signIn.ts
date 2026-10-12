/** The system browser gives a bound Sign-In Link to the web view. */
import type { ServerAddress } from './address'

interface SignInShell {
  authenticate(options: { url: string }): Promise<{ url: string }>
  open(address: ServerAddress & { verifier?: string }): Promise<void>
}

function base64url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

export async function openConnectedServer(
  address: ServerAddress,
  platform: string,
  shell: SignInShell,
  random: Crypto = crypto,
): Promise<void> {
  if (address.opens !== address.origin) return shell.open(address)
  if (platform !== 'ios' && platform !== 'android') throw new Error('Sign-in needs iOS or Android.')
  const verifier = base64url(random.getRandomValues(new Uint8Array(32)))
  const state = base64url(random.getRandomValues(new Uint8Array(32)))
  const challenge = base64url(new Uint8Array(await random.subtle.digest('SHA-256', new TextEncoder().encode(verifier))))
  const page = new URL('/sign-in', address.origin)
  page.search = new URLSearchParams({ app: platform, state, challenge }).toString()
  const answer = await shell.authenticate({ url: page.href })
  const callback = new URL(answer.url)
  if (callback.protocol !== 'pagis-app:' || callback.hostname !== 'signed-in' || callback.pathname !== '' || callback.username || callback.password || callback.port) {
    throw new Error('This is not the sign-in callback of Pagis.')
  }
  if (callback.searchParams.getAll('state').length !== 1 || callback.searchParams.get('state') !== state) {
    throw new Error('The sign-in callback has the wrong state. Connect again.')
  }
  if (callback.hash.length <= 1) throw new Error('The sign-in callback holds no secret. Connect again.')
  await shell.open({ ...address, opens: new URL(`/sign-in${callback.hash}`, address.origin).href, verifier })
}
