// The server's own first run.
//
// A server boots with an Org and a Workspace and nobody who can sign in:
// the seeded person has no address and no password. This page takes the
// first address, the first password and the installation's provider
// keys, and the answer signs the new administrator in.
//
// It is one of the two routes of this port that answer without a signed
// in administrator, and it answers only while nobody can sign in. From
// the first password onwards the daemon says `410 Gone` and this page is
// never shown again.

import { useState } from 'react'

import type { ApiClient } from '../api/client'
import { Button, Frame, Input, Row, SectionLabel } from '../primitives'
import { providerName } from '../providers'
import { errorMessage, useCompleteSetup } from '../queries'

/** The floor the daemon enforces, stated before the person types. */
const MIN_PASSWORD_LENGTH = 12

export function Setup({
  api,
  providers,
  configured,
}: {
  api: ApiClient
  /** The provider ids the flow may take a key for. */
  providers: string[]
  /** The ones that already hold a key, from the environment or the
   *  config file. */
  configured: string[]
}) {
  const complete = useCompleteSetup(api)
  const [email, setEmail] = useState('')
  const [name, setName] = useState('')
  const [password, setPassword] = useState('')
  const [keys, setKeys] = useState<Record<string, string>>({})
  const ready = email.includes('@') && password.length >= MIN_PASSWORD_LENGTH

  return (
    <div className="administration">
      <header className="administration-header">
        <h1>Set up this installation</h1>
        <p className="administration-note">
          Nobody can sign in to this server yet. The account you make here is its
          first administrator, and the keys you give are the installation's:
          every person you create later thinks on them.
        </p>
      </header>
      <form
        className="administration-section"
        aria-label="Set up this installation"
        onSubmit={(event) => {
          event.preventDefault()
          complete.mutate({
            email: email.trim(),
            password,
            name: name.trim() === '' ? undefined : name.trim(),
            provider_keys: Object.fromEntries(
              Object.entries(keys)
                .map(([provider, key]) => [provider, key.trim()])
                .filter(([, key]) => key !== ''),
            ),
          })
        }}
      >
        <SectionLabel>The first administrator</SectionLabel>
        <Frame hint={`A password is at least ${MIN_PASSWORD_LENGTH} characters. You sign in with this address from now on, here and in Pagis itself.`}>
          <Row>
            <Input
              type="email"
              aria-label="Email address"
              placeholder="ada@example.net"
              autoComplete="username"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
            />
            <Input
              aria-label="Name"
              placeholder="Name the sprites call you"
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
            <Input
              type="password"
              aria-label="Password"
              placeholder="Password"
              autoComplete="new-password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </Row>
        </Frame>

        <SectionLabel>Provider keys</SectionLabel>
        <Frame hint="A key goes to the installation's secret store and is never read back. A provider with no key is skipped, and a sprite with no key at all cannot think.">
          {providers.map((provider) => (
            <Row key={provider}>
              <span className="administration-key">
                {providerName(provider)}
              </span>
              {configured.includes(provider) ? (
                <span className="administration-note">
                  already set, from the environment or the config file
                </span>
              ) : (
                <Input
                  type="password"
                  aria-label={`${providerName(provider)} key`}
                  placeholder="Key"
                  autoComplete="off"
                  value={keys[provider] ?? ''}
                  onChange={(event) =>
                    setKeys((current) => ({ ...current, [provider]: event.target.value }))
                  }
                />
              )}
            </Row>
          ))}
        </Frame>

        <Row>
          <Button type="submit" variant="primary" disabled={!ready || complete.isPending}>
            Make this administrator
          </Button>
          {complete.isError && (
            <span className="administration-note" role="alert">
              {errorMessage(complete.error, 'That did not go through.')}
            </span>
          )}
        </Row>
      </form>
    </div>
  )
}
