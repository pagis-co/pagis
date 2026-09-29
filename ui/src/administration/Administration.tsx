// The Administration Interface: what an administrator reads and
// changes about the installation, served on the administration port.
//
// The port answers nothing to a person who is not a signed-in
// administrator, so this page has four states and no fifth: the
// first-run setup of a server nobody can sign in to yet, the sign-in for
// a browser with no Session, a sentence for a person who is not an
// administrator, and the interface itself. A member gets nothing, not a
// partial view.
//
// The installation's people, spend, providers, settings and Plugins
// answer on this port alone, so this page is the one surface that draws them. The
// product links here for an administrator and draws none of them.

import { useMemo, useState } from 'react'

import { createApiClient, type ApiClient } from '../api/client'
import { SignIn } from '../SignIn'
import { Plugins } from '../components/Plugins'
import { People } from '../components/settings/People'
import { SystemSection } from '../components/settings/SystemSection'
import { Button, cx } from '../primitives'
import { errorCode, useSetupState, useSignOut, useUser, useUserName } from '../queries'
import { Health } from './Health'
import { Providers } from './Providers'
import { Resources } from './Resources'
import { Sessions } from './Sessions'
import { Setup } from './Setup'
import { Spend } from './Spend'

/** Each view of the interface, in the order the nav shows them. */
export type AdministrationView =
  | 'spend'
  | 'people'
  | 'sessions'
  | 'resources'
  | 'providers'
  | 'settings'
  | 'plugins'
  | 'health'

export const ADMINISTRATION_VIEWS: { value: AdministrationView; label: string }[] = [
  { value: 'spend', label: 'Spend' },
  { value: 'people', label: 'People' },
  { value: 'sessions', label: 'Sessions' },
  { value: 'resources', label: 'Resources' },
  { value: 'providers', label: 'Providers' },
  { value: 'settings', label: 'Settings' },
  { value: 'plugins', label: 'Plugins' },
  { value: 'health', label: 'Health' },
]

/** The view one address opens. Every view is an address of this port, so
 *  a bookmark of one opens it again. */
export function viewOfPath(path: string): AdministrationView {
  const name = path.replace(/^\/+/, '').split('/')[0]
  const view = ADMINISTRATION_VIEWS.find((entry) => entry.value === name)
  return view?.value ?? 'spend'
}

export function AdministrationShell({
  api,
  initialView,
}: {
  api: ApiClient
  initialView: AdministrationView
}) {
  const [view, setView] = useState<AdministrationView>(initialView)
  const signOut = useSignOut(api)
  const name = useUserName(api)

  return (
    <div className="administration">
      <header className="administration-header">
        <div className="administration-header-top">
          <h1>Administration</h1>
          <span className="administration-note">{name}</span>
          <Button
            variant="ghost"
            className="administration-row-trailing"
            disabled={signOut.isPending}
            onClick={() => signOut.mutate()}
          >
            Sign out
          </Button>
        </div>
        <nav aria-label="Administration">
          {ADMINISTRATION_VIEWS.map((entry) => (
            <Button
              key={entry.value}
              variant="ghost"
              className={cx(
                'administration-nav-item',
                entry.value === view && 'administration-nav-item-active',
              )}
              aria-current={entry.value === view ? 'page' : undefined}
              onClick={() => {
                setView(entry.value)
                window.history.pushState(null, '', `/${entry.value}`)
              }}
            >
              {entry.label}
            </Button>
          ))}
        </nav>
      </header>
      <main className="administration-content">
        {view === 'spend' && <Spend api={api} />}
        {view === 'people' && <People api={api} />}
        {view === 'sessions' && <Sessions api={api} />}
        {view === 'resources' && <Resources api={api} />}
        {view === 'providers' && <Providers api={api} />}
        {view === 'settings' && (
          <section className="administration-settings">
            <SystemSection api={api} />
          </section>
        )}
        {view === 'plugins' && <Plugins api={api} />}
        {view === 'health' && <Health api={api} />}
      </main>
    </div>
  )
}

export function Administration({ api: given }: { api?: ApiClient } = {}) {
  const api = useMemo(() => given ?? createApiClient(), [given])
  // The server's own first run comes before everything else: a
  // server nobody can sign in to has a setup page and nothing else, and
  // the page is gone from the first password onwards.
  const setup = useSetupState(api)
  const user = useUser(api)

  if (setup.isPending) return null
  if (setup.data !== undefined && setup.data !== null) {
    return (
      <Setup
        api={api}
        providers={setup.data.providers}
        configured={setup.data.configured_providers}
      />
    )
  }
  if (user.isPending) return null
  // The port refuses a member outright, so the page says so rather than
  // showing an interface with nothing in it.
  if (user.data === undefined) {
    if (errorCode(user.error) === 'forbidden') {
      return (
        <div className="administration-refused" role="alert">
          <h1>Administration</h1>
          <p>
            This installation's administration is for its administrators. Your
            account is a member, so there is nothing here for you. Pagis itself
            is on the product port.
          </p>
        </div>
      )
    }
    return <SignIn api={api} />
  }
  return (
    <AdministrationShell api={api} initialView={viewOfPath(window.location.pathname)} />
  )
}
