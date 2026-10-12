/** The system browser signs the Person in and returns a bound link. */
import { useEffect, useRef, useState } from 'react'
import type { ApiClient } from './api/client'
import { useSetupState, useSignInMethod, useUser } from './queries'
import { SignIn } from './SignIn'

function appSignInRequest() {
  const query = new URLSearchParams(window.location.search)
  const app = query.get('app')
  const state = query.get('state')
  const challenge = query.get('challenge')
  if ((app !== 'ios' && app !== 'android' && app !== 'desktop') || !state || state.length > 256 ||
    !challenge || !/^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/.test(challenge) ||
    ['app', 'state', 'challenge'].some((key) => query.getAll(key).length !== 1)) return null
  return { app, state, challenge }
}

export function BrowserSignInPage({ api, redirect = (url) => window.location.assign(url) }: {
  api: ApiClient
  redirect?: (url: string) => void
}) {
  const [request] = useState(appSignInRequest)
  if (request === null) return <p role="alert">This app sign-in address is not complete. Connect again in the app.</p>
  return <BrowserSignIn api={api} request={request} redirect={redirect} />
}

function BrowserSignIn({ api, request, redirect }: {
  api: ApiClient
  request: { app: string; state: string; challenge: string }
  redirect: (url: string) => void
}) {
  const user = useUser(api)
  const method = useSignInMethod(api)
  const setup = useSetupState(api)
  const started = useRef(false)
  const [callback, setCallback] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)

  useEffect(() => {
    if (!user.data || started.current) return
    started.current = true
    void (async () => {
      try {
        let secret: string
        try {
          const link = await api.POST('/api/v1/settings/sign-in-links', { body: { challenge: request.challenge } })
          if (!link.data) throw new Error('Pagis could not make a sign-in link. Connect again in the app.')
          secret = new URL(link.data.url).hash
          if (secret.length <= 1) throw new Error('The sign-in link holds no secret. Connect again in the app.')
        } finally {
          const ended = await api.DELETE('/api/v1/sessions/current')
          if (!ended.response.ok) throw new Error('Pagis could not end the browser Session. Connect again in the app.')
        }
        const url = `${request.app === 'desktop' ? 'pagis://auth' : 'pagis-app://signed-in'}?${new URLSearchParams({ state: request.state })}${secret}`
        setCallback(url)
        try {
          redirect(url)
        } catch {
          // The link stays visible when the browser refuses the custom scheme.
        }
      } catch (error) {
        setFailure(error instanceof Error ? error.message : 'Sign-in failed. Connect again in the app.')
      }
    })()
  }, [api, user.data, request, redirect])

  if (failure) return <p role="alert">{failure}</p>
  if (callback) return <div className="sign-in"><div className="sign-in-card"><a href={callback}>Return to the app</a></div></div>
  if (user.data) return <p role="status">Returning to the app…</p>
  if (user.isPending || method.isPending) return null
  return <SignIn api={api} method={method.data ?? 'password'} administrationOrigin={setup.data?.administration_origin ?? null} />
}
