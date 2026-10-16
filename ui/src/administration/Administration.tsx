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

import { useEffect, useMemo, useState } from 'react'

import { createApiClient, type ApiClient } from '../api/client'
import { SignIn } from '../SignIn'
import { Plugins } from '../components/Plugins'
import { Workspaces } from '../components/settings/Workspaces'
import { People } from '../components/settings/People'
import { SystemSection } from '../components/settings/SystemSection'
import { ReadingColumn } from '../primitives'
import { SidebarFrame } from '../components/sidebar/SidebarFrame'
import { SettingsNav } from '../components/settings/SettingsNav'
import { NavBar } from '../components/phone/TopBar'
import { useIsMobile } from '../primitives/useMediaQuery'
import { errorCode, useAdministrationSections, useSetupState, useSignOut, useUser, useUserName } from '../queries'
import { PersonDefaults } from './PersonDefaults'
import { Health } from './Health'
import { Providers } from './Providers'
import { Resources } from './Resources'
import { Sessions } from './Sessions'
import { Setup } from './Setup'
import { Spend } from './Spend'

import { ADMINISTRATION_VIEWS, type AdministrationView } from './views'

/** The view one address opens. Every view is an address of this port, so
 *  a bookmark of one opens it again. */
export function viewOfPath(path: string, pathPrefix = ''): AdministrationView {
  if (pathPrefix !== '' && path !== pathPrefix && !path.startsWith(`${pathPrefix}/`)) return 'spend'
  const name = path.slice(pathPrefix.length).replace(/^\/+/, '').split('/')[0]
  const view = ADMINISTRATION_VIEWS.find((entry) => entry.value === name)
  return view?.value ?? 'spend'
}

export function AdministrationShell({
  api,
  initialView,
  pathPrefix,
}: {
  api: ApiClient
  initialView: AdministrationView
  pathPrefix: string
}) {
  const [selected, setView] = useState<AdministrationView>(initialView)
  const sections = useAdministrationSections(api)
  const views = (sections.data?.sections ?? []).flatMap((section) => {
    const entry = ADMINISTRATION_VIEWS.find((entry) => entry.value === section)
    return entry ? [entry] : []
  })
  const view = views.find((entry) => entry.value === selected)?.value ?? views[0]?.value
  const signOut = useSignOut(api)
  const name = useUserName(api)
  const user = useUser(api).data
  const phone = useIsMobile()

  useEffect(() => {
    if (view && view !== selected) {
      setView(view)
      window.history.replaceState(null, '', `${pathPrefix}/${view}`)
    }
  }, [view, selected, pathPrefix])

  useEffect(() => {
    const readPath = () => setView(viewOfPath(window.location.pathname, pathPrefix))
    window.addEventListener('popstate', readPath)
    return () => window.removeEventListener('popstate', readPath)
  }, [pathPrefix])

  if (sections.isPending) return null
  if (sections.isError) return <p role="alert">The administration sections could not be read.</p>

  return (
    <div className={phone ? 'administration phone-shell' : 'administration'}>
      {phone ? (
        <NavBar back={{ label: 'Settings', onBack: () => window.location.assign(`${sections.data.public_origin}/settings`) }} />
      ) : <>
        <SidebarFrame
          userName={name}
          org={user?.org}
          links={user?.product_app_links}
          role={user?.role ?? 'administrator'}
          notice={user?.notice}
          currentPlace="settings"
          publicOrigin={sections.data.public_origin}
          onSignOut={() => signOut.mutate()}
          signOutPending={signOut.isPending}
        />
        {user?.administration && <SettingsNav
          isAdministrator
          // The Person is on this page already, so the group shows no tunnel command.
          administration={{ ...user.administration, loopback: false, sections: sections.data.sections, links: sections.data.links }}
          publicOrigin={sections.data.public_origin}
          currentView={view}
          onSelectView={(next) => {
            if (next === view) return
            setView(next)
            window.history.pushState(null, '', `${pathPrefix}/${next}`)
          }}
        />}
      </>}
      <main className="administration-content">
        <ReadingColumn>
          {view === 'spend' && <Spend api={api} />}
          {view === 'people' && <People api={api} />}
          {view === 'workspaces' && <Workspaces api={api} />}
          {view === 'person-defaults' && <PersonDefaults api={api} />}
          {view === 'sessions' && <Sessions api={api} hostsAvailable={sections.data.sections.includes('hosts')} />}
          {view === 'resources' && <Resources api={api} />}
          {view === 'providers' && <Providers api={api} />}
          {view === 'settings' && (
            <section className="administration-settings">
              <SystemSection api={api} restartAvailable={sections.data.sections.includes('restart')} />
            </section>
          )}
          {view === 'plugins' && <Plugins api={api} />}
          {view === 'health' && <Health api={api} />}
        </ReadingColumn>
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
  const pathPrefix = user.data.administration!.path_prefix
  return (
    <AdministrationShell
      api={api}
      pathPrefix={pathPrefix}
      initialView={viewOfPath(window.location.pathname, pathPrefix)}
    />
  )
}
