import { RouterProvider, type RouterHistory } from '@tanstack/react-router'
import { useEffect, useMemo, useRef } from 'react'

import { createApiClient } from './api/client'
import type { ApiClient } from './api/client'
import { useSetupState, useUser } from './queries'
import { createAppRouter } from './routes'
import { SignIn } from './SignIn'

/** The app is its router: every view has a URL. The session is
 *  an HTTP-only cookie, so the app asks the daemon who is
 *  signed in: a refused answer gives the screen to the sign-in page.
 *  Tests pass a memory history; the browser gets the default one. */
export function App({ history }: { history?: RouterHistory }) {
  const api = useMemo(() => createApiClient(), [])
  const router = useMemo(() => createAppRouter({ api }, history), [api, history])
  const user = useUser(api)
  const signedIn = user.data !== undefined
  // An address belongs to the session that opened it. When the session
  // ends, the address goes back to Home, so the next sign-in opens Home
  // and not the last person's conversation. A page that opens with no
  // session keeps its address: the sign-in opens it, and a conversation
  // that is not the person's says that it does not exist.
  const hadSession = useRef(false)
  useEffect(() => {
    if (signedIn) {
      hadSession.current = true
    } else if (hadSession.current) {
      hadSession.current = false
      void router.navigate({ to: '/', replace: true })
    }
  }, [signedIn, router])
  if (user.isPending) return null
  if (user.data === undefined) return <ProductSignIn api={api} />
  return <RouterProvider router={router} />
}

/** The sign-in of the product port. The setup read answers only while
 *  nobody can sign in, and then the page names where the first
 *  Administrator is made. */
function ProductSignIn({ api }: { api: ApiClient }) {
  const setup = useSetupState(api)
  return (
    <SignIn api={api} administrationOrigin={setup.data?.administration_origin ?? null} />
  )
}
