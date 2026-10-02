// The sign-in page: the person gives their address and their
// password, and the daemon answers with a session cookie. The page
// keeps no credential and no token of its own. On a server where nobody
// can sign in yet, the page names where the first Administrator is
// made: the Administration Port, which binds loopback, so the page
// names it and does not link to it.
//
// In Remote Access (ADR-0028) the daemon takes no password from another
// machine, so a browser there gets one field for a Sign-In Link instead,
// and the line that says where to get one. The field takes the whole
// link or its secret, and spends it through the trade of the `/sign-in`
// page.

import { useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import type { ApiClient, SignInMethod } from './api/client'
import { Button, Input, LogoMark } from './primitives'
import { useLinkSignIn, userKey, useSignIn } from './queries'

import './sign-in.css'

/** Where a person gets a new Sign-In Link. The daemon says the same when
 *  it refuses a link. */
export const WHERE_TO_GET_A_LINK =
  'Make a new link in Settings → Sessions on a browser or app that is signed in. ' +
  'Or ask an Administrator for a new invite, or run "pagis pair" on the machine of the server.'

/** The secret of a pasted Sign-In Link: what follows `#` in the whole
 *  link, `<public origin>/sign-in#<secret>`, or the pasted text where it
 *  is the secret alone. */
export function linkSecret(pasted: string): string {
  const text = pasted.trim()
  const hash = text.indexOf('#')
  if (hash === -1) return text
  const secret = text.slice(hash + 1)
  try {
    return decodeURIComponent(secret)
  } catch {
    return secret
  }
}

export function SignIn({
  api,
  administrationOrigin = null,
  method = 'password',
}: {
  api: ApiClient
  /** The Administration Port's origin while nobody can sign in, else
   *  `null`. */
  administrationOrigin?: string | null
  /** How this browser signs in, as the daemon says. */
  method?: SignInMethod
}) {
  return (
    <div className="sign-in">
      {method === 'link' ? (
        <LinkForm api={api} administrationOrigin={administrationOrigin} />
      ) : (
        <PasswordForm api={api} administrationOrigin={administrationOrigin} />
      )}
    </div>
  )
}

function Title() {
  return (
    <h1 className="sign-in-title">
      <LogoMark />
      Pagis
    </h1>
  )
}

function NoAdministrator({ administrationOrigin }: { administrationOrigin: string | null }) {
  if (administrationOrigin === null) return null
  return (
    <p className="sign-in-setup" role="status">
      No Administrator exists yet, so nobody can sign in. On the server, open{' '}
      <code>{administrationOrigin}/</code> to make the first Administrator,
      or set <code>PAGIS_ADMIN_EMAIL</code> and <code>PAGIS_ADMIN_PASSWORD</code> and
      start Pagis again.
    </p>
  )
}

function PasswordForm({
  api,
  administrationOrigin,
}: {
  api: ApiClient
  administrationOrigin: string | null
}) {
  const signIn = useSignIn(api)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')

  return (
    <form
      className="sign-in-card"
      aria-label="Sign in"
      onSubmit={(event) => {
        event.preventDefault()
        signIn.mutate({ email, password })
      }}
    >
      <Title />
      <NoAdministrator administrationOrigin={administrationOrigin} />
      <label className="sign-in-field">
        <span>Email</span>
        <Input
          type="email"
          autoComplete="username"
          value={email}
          onChange={(event) => setEmail(event.target.value)}
        />
      </label>
      <label className="sign-in-field">
        <span>Password</span>
        <Input
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
        />
      </label>
      {signIn.isError && (
        <p className="sign-in-error" role="alert">
          {signIn.error.message}
        </p>
      )}
      <Button type="submit" variant="primary" disabled={signIn.isPending}>
        Sign in
      </Button>
    </form>
  )
}

function LinkForm({
  api,
  administrationOrigin,
}: {
  api: ApiClient
  administrationOrigin: string | null
}) {
  const signIn = useLinkSignIn(api)
  const queryClient = useQueryClient()
  const [pasted, setPasted] = useState('')
  const secret = linkSecret(pasted)

  return (
    <form
      className="sign-in-card"
      aria-label="Sign in"
      onSubmit={(event) => {
        event.preventDefault()
        // The answer set the cookie, and the person it names opens the
        // app at the address this page opened at.
        signIn.mutate(secret, {
          onSuccess: (user) => queryClient.setQueryData(userKey, user),
        })
      }}
    >
      <Title />
      <NoAdministrator administrationOrigin={administrationOrigin} />
      <label className="sign-in-field">
        <span>Paste a sign-in link</span>
        <Input
          autoComplete="off"
          spellCheck={false}
          value={pasted}
          onChange={(event) => setPasted(event.target.value)}
        />
      </label>
      <p className="sign-in-setup">{WHERE_TO_GET_A_LINK}</p>
      {signIn.isError && (
        <p className="sign-in-error" role="alert">
          {signIn.error.message}
        </p>
      )}
      <Button type="submit" variant="primary" disabled={signIn.isPending || secret === ''}>
        Sign in
      </Button>
    </form>
  )
}
