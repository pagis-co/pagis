// The sign-in page: the person gives their address and their
// password, and the daemon answers with a session cookie. The page
// keeps no credential and no token of its own. On a server where nobody
// can sign in yet, the page names where the first Administrator is
// made: the Administration Port, which binds loopback, so the page
// names it and does not link to it.

import { useState } from 'react'

import type { ApiClient } from './api/client'
import { Button, Input, LogoMark } from './primitives'
import { useSignIn } from './queries'

import './sign-in.css'

export function SignIn({
  api,
  administrationOrigin = null,
}: {
  api: ApiClient
  /** The Administration Port's origin while nobody can sign in, else
   *  `null`. */
  administrationOrigin?: string | null
}) {
  const signIn = useSignIn(api)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')

  return (
    <div className="sign-in">
      <form
        className="sign-in-card"
        aria-label="Sign in"
        onSubmit={(event) => {
          event.preventDefault()
          signIn.mutate({ email, password })
        }}
      >
        <h1 className="sign-in-title">
          <LogoMark />
          Pagis
        </h1>
        {administrationOrigin !== null && (
          <p className="sign-in-setup" role="status">
            No Administrator exists yet, so nobody can sign in. On the server, open{' '}
            <code>{administrationOrigin}/</code> to make the first Administrator,
            or set <code>PAGIS_ADMIN_EMAIL</code> and <code>PAGIS_ADMIN_PASSWORD</code> and
            start Pagis again.
          </p>
        )}
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
    </div>
  )
}
