// The page a Sign-In Link opens: `<public origin>/sign-in#<secret>`
// (ADR-0028). The secret is in the fragment, which a browser sends to
// no server, so opening the link spends nothing: a message app that
// opens it to make a preview runs no script and posts nothing. This page
// takes the secret out of the address bar, posts it, and opens the app
// once the daemon answers with a Session cookie.

import { useEffect, useRef, useState } from 'react'

import type { ApiClient } from './api/client'
import { LogoMark } from './primitives'
import { useLinkSignIn } from './queries'

import './sign-in.css'

/** The path of the page. */
export const SIGN_IN_LINK_PATH = '/sign-in'

/** Open the app at its root. The answer set the cookie, so the app
 *  reads the person on its first request. The page leaves the history,
 *  so Back does not open it again. */
function openTheApp() {
  window.location.replace('/')
}

export function SignInLinkPage({
  api,
  onSignedIn = openTheApp,
}: {
  api: ApiClient
  /** What happens once the daemon signed the browser in. */
  onSignedIn?: () => void
}) {
  const signIn = useLinkSignIn(api)
  const [missing, setMissing] = useState(false)
  const started = useRef(false)

  useEffect(() => {
    // The page posts the secret once, also where React runs an effect
    // twice in development.
    if (started.current) return
    started.current = true
    const secret = decodeURIComponent(window.location.hash.replace(/^#/, ''))
    // The secret leaves the address bar before anything else, so the
    // browser history does not keep it.
    window.history.replaceState(null, '', SIGN_IN_LINK_PATH)
    if (secret === '') {
      setMissing(true)
      return
    }
    signIn.mutate(secret)
  }, [signIn])

  useEffect(() => {
    if (signIn.isSuccess) onSignedIn()
  }, [signIn.isSuccess, onSignedIn])

  const failure = missing
    ? 'This address holds no sign-in link. Open the whole link again.'
    : signIn.isError
      ? signIn.error.message
      : null

  return (
    <div className="sign-in">
      <div className="sign-in-card">
        <h1 className="sign-in-title">
          <LogoMark />
          Pagis
        </h1>
        {failure === null ? (
          <p className="sign-in-setup" role="status">
            Signing you in…
          </p>
        ) : (
          <>
            <p className="sign-in-error" role="alert">
              {failure}
            </p>
            {/* The sign-in page takes a password, or a pasted link where
                Remote Access takes no password from this machine. */}
            <a href="/">Go to the sign-in page</a>
          </>
        )}
      </div>
    </div>
  )
}
