// The route tree. Every view has a URL: a channel, a thread, a
// run, a Coding Session, an agent, each Settings section, and the
// Automations and Software destinations. The durable inspector tenant (the Desk
// panel) rides in the `panel` search parameter; Call and Mail stay
// transient in their stores (ADR-0022).

import {
  Outlet,
  createRootRouteWithContext,
  createRoute,
  createRouter,
  useParams,
  useNavigate,
  useRouteContext,
  type RouterHistory,
} from '@tanstack/react-router'
import { useEffect } from 'react'

import type { ApiClient } from './api/client'
import { AppShell } from './AppShell'
import { ConversationsPhone } from './components/phone/ConversationsPhone'
import { You } from './components/phone/You'
import { SettingsPhone } from './components/phone/SettingsPhone'
import { KeypadCodeRow } from './components/TrustedContacts'
import { NavBar } from './components/phone/TopBar'
import { ThreadPane } from './components/ThreadPane'
import { CallInspector } from './components/CallInspector'
import { CallPhone } from './components/phone/CallPhone'
import { RunPhone } from './components/phone/RunPhone'
import { DeskPhone } from './components/phone/DeskPhone'
import { Automations } from './components/Automations'
import { CodingSessionPage } from './components/coding/CodingSessionPage'
import { ChannelComposer } from './components/composer/ChannelComposer'
import { Home } from './components/home/Home'
import { MemoryPage } from './components/memory/MemoryPage'
import type { MemoryView } from './components/memory/pages'
import { RunTimeline } from './components/runs/RunTimeline'
import { RunsList } from './components/runs/RunsList'
import {
  SettingsPanel,
  SettingsShell,
  isAdministratorSection,
  isSettingsSection,
  visibleSettingsSections,
} from './components/SettingsPanel'
import { ConnectionPage } from './components/connection/ConnectionPage'
import { MissingConversation } from './components/MissingConversation'
import { directMessageChannel } from './components/AskAnAgent'
import { chiefOfStaff } from './components/sidebar/conversations'
import { AgentProfile } from './components/sprites/AgentProfile'
import { AgentAccessPhone } from './components/sprites/AgentAccessPhone'
import { AgentMemory } from './components/sprites/AgentMemory'
import { AgentWork } from './components/sprites/AgentWork'
import { SpriteRoster } from './components/sprites/SpriteRoster'
import { Software } from './components/Software'
import { ThreadHeader } from './components/ThreadHeader'
import { Timeline } from './components/Timeline'
import {
  errorCode,
  useAgents,
  useChannels,
  useIsAdministrator,
  useTimeline,
  useTrustList,
  useWorkspace,
} from './queries'
import { openDocument } from './navigation'
import { useCallInspector, useMailInspector } from './state/stores'
import { useIsCompact, useIsMobile } from './state/useIsMobile'

export interface RouterContext {
  api: ApiClient
}

/** The inspector slot tenant that is durable enough to be a URL. */
export type Panel = 'desk'

export interface PanelSearch {
  panel?: Panel
  request?: string
  from?: string
}

const rootRoute = createRootRouteWithContext<RouterContext>()({
  component: AppShell,
  // The search is the shell's, so it is validated once, at the root.
  // An unknown parameter does not survive a navigation.
  validateSearch: (search: Record<string, unknown>): PanelSearch =>
    ({ ...(search.panel === 'desk' ? { panel: 'desk' as const } : {}), ...(typeof search.request === 'string' ? { request: search.request } : {}), ...(typeof search.from === 'string' ? { from: search.from } : {}) }),
})

function useApi(): ApiClient {
  return useRouteContext({ from: '__root__' }).api
}

/** Home: the first place and the landing view (ADR-0022). It
 *  is the Report of the Chief of Staff: what needs the user, the brief
 *  itself, and the work record under it. The Desks sit in the panel
 *  beside it. */
function HomeView() {
  const api = useApi()
  const navigate = useNavigate()
  const panel = indexRoute.useSearch().panel
  const closeCall = useCallInspector((state) => state.close)
  const closeMail = useMailInspector((state) => state.close)
  // On a wide screen Home keeps its Desk Panel open by the route
  // (ADR-0022). At 1100px and below the person opens and closes it.
  const isCompact = useIsCompact()
  const toggleDesk = () => {
    closeCall()
    closeMail()
    void navigate({ to: '/', search: panel === 'desk' ? {} : { panel: 'desk' } })
  }
  return (
    <Home
      api={api}
      desk={isCompact ? { open: panel === 'desk', onToggle: toggleDesk } : null}
      onOpenChannel={(channelId) =>
        void navigate({ to: '/c/$channelId', params: { channelId }, search: {} })
      }
      onOpenRun={(runId) => void navigate({ to: '/runs/$runId', params: { runId } })}
    />
  )
}

const indexRoute = createRoute({
  staticData: { phone: 'tab' },
  getParentRoute: () => rootRoute,
  path: '/',
  component: HomeView,
})

/** Sprites: the roster of every Agent. `?new=1` opens the creating
 *  form, so a link and the command palette both reach it. */
function SpritesView() {
  const api = useApi()
  const navigate = useNavigate()
  const { new: creating } = spritesRoute.useSearch()
  return (
    <SpriteRoster
      api={api}
      creating={creating === '1'}
      onCreating={(next) =>
        void navigate({ to: '/sprites', search: next ? { new: '1' } : {} })
      }
      onOpenAgent={(agentId) =>
        void navigate({ to: '/sprites/$agentId', params: { agentId } })
      }
      onOpenChannel={(channelId) =>
        void navigate({ to: '/c/$channelId', params: { channelId }, search: {} })
      }
    />
  )
}

export interface SpritesSearch {
  new?: '1'
}

const spritesRoute = createRoute({
  staticData: { phone: 'tab' },
  getParentRoute: () => rootRoute,
  path: '/sprites',
  component: SpritesView,
  validateSearch: (search: Record<string, unknown>): SpritesSearch =>
    search.new === '1' || search.new === 1 ? { new: '1' } : {},
})

/** `/c` holds no view of its own: it lands on the first channel. */
function ConversationsIndexView() {
  const api = useApi()
  const channels = useChannels(api)
  const navigate = useNavigate()
  const first = channels.data?.[0]?.id

  useEffect(() => {
    if (first !== undefined) {
      void navigate({
        to: '/c/$channelId',
        params: { channelId: first },
        replace: true,
      })
    }
  }, [first, navigate])

  if (channels.data !== undefined && channels.data.length === 0) {
    return <div className="timeline-empty">Create a channel to start.</div>
  }
  return <div className="timeline-empty" />
}

const conversationsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/c',
})

const conversationsIndexRoute = createRoute({
  getParentRoute: () => conversationsRoute,
  path: '/',
  component: ConversationsIndexView,
})

function ConversationView() {
  const api = useApi()
  const phone = useIsMobile()
  const { messageId } = useParams({ strict: false }) as { messageId?: string }
  const { channelId } = channelRoute.useParams()
  const navigate = useNavigate()
  const panel = channelRoute.useSearch().panel
  // On a wide screen the Chief of Staff's own channel keeps its Desk
  // Panel open by the route (ADR-0022), so the header offers no toggle
  // for it.
  const agents = useAgents(api)
  const workspace = useWorkspace(api)
  const channels = useChannels(api)
  const chief = chiefOfStaff(
    agents.data ?? [],
    workspace.data?.chief_of_staff_agent_id,
  )
  const isCompact = useIsCompact()
  const panelIsRouteOwned =
    !isCompact &&
    chief !== null &&
    directMessageChannel(channels.data ?? [], chief.id) === channelId
  const closeCall = useCallInspector((state) => state.close)
  const closeMail = useMailInspector((state) => state.close)
  // The daemon answers a channel that is not the person's as not
  // found, the same as a channel that never was.
  const timeline = useTimeline(api, channelId)

  // One slot, one tenant (ADR-0022): the Desk panel takes the slot
  // back from a Call or a Mail.
  const togglePanel = (next: Panel) => {
    closeCall()
    closeMail()
    void navigate({
      to: '/c/$channelId',
      params: { channelId },
      search: panel === next ? {} : { panel: next },
    })
  }

  if (errorCode(timeline.error) === 'not_found') {
    return (
      <MissingConversation
        onOpenHome={() => void navigate({ to: '/' })}
      />
    )
  }

  if (phone && messageId) return <><NavBar back={{ label: 'Conversation', onBack: () => void navigate({ to: '/c/$channelId', params: { channelId } }) }} /><ThreadPane api={api} channelId={channelId} rootId={messageId} onClose={() => void navigate({ to: '/c/$channelId', params: { channelId } })} onOpenDesk={() => togglePanel('desk')} /></>

  return (
    <>
      <ThreadHeader
        key={channelId}
        api={api}
        channelId={channelId}
        panel={panel}
        panelIsRouteOwned={panelIsRouteOwned}
        onTogglePanel={togglePanel}
      />
      <Timeline
        api={api}
        channelId={channelId}
        onOpenThread={(rootId) =>
          void navigate({
            to: '/c/$channelId/t/$messageId',
            params: { channelId, messageId: rootId },
          })
        }
        onOpenChannel={(next) =>
          void navigate({ to: '/c/$channelId', params: { channelId: next }, search: {} })
        }
        onOpenDesk={() => {
          closeCall()
          closeMail()
          void navigate({
            to: '/c/$channelId',
            params: { channelId },
            search: { panel: 'desk' },
          })
        }}
      />
      <ChannelComposer api={api} channelId={channelId} />
      {/* The thread route renders nothing of its own: the thread pane
          is a tenant of the shell's inspector slot. */}
      <Outlet />
    </>
  )
}

const channelRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => conversationsRoute,
  path: '$channelId',
  component: ConversationView,
})

const threadRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => channelRoute,
  path: '/t/$messageId',
  component: () => null,
})

/** `/runs`: the work record, grouped by day. */
function RunsView() {
  const api = useApi()
  const navigate = useNavigate()
  return (
    <RunsList
      api={api}
      onOpenRun={(runId) => void navigate({ to: '/runs/$runId', params: { runId } })}
    />
  )
}

const runsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/runs',
})

const runsIndexRoute = createRoute({
  getParentRoute: () => runsRoute,
  path: '/',
  component: RunsView,
})

/** `/runs/:runId`: one run as a timeline. */
function RunView() {
  const api = useApi()
  const { runId } = runRoute.useParams()
  const navigate = useNavigate()
  const phone = useIsMobile()
  return phone ? <RunPhone api={api} runId={runId} /> : <RunTimeline api={api} runId={runId} onBack={() => void navigate({ to: '/runs' })} />
}

const runRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => runsRoute,
  path: '$runId',
  component: RunView,
})

const codingRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/coding',
})

/** `/coding/:sessionId`: one Coding Session as its transcript, its
 *  plan and its tool calls. */
function CodingSessionView() {
  const api = useApi()
  const { sessionId } = codingSessionRoute.useParams()
  return <CodingSessionPage api={api} sessionId={sessionId} />
}

const codingSessionRoute = createRoute({
  getParentRoute: () => codingRoute,
  path: '$sessionId',
  component: CodingSessionView,
})

/** One sprite: the profile page. */
function AgentView() {
  const api = useApi()
  const { agentId } = agentRoute.useParams()
  const navigate = useNavigate()
  return (
    <AgentProfile
      api={api}
      agentId={agentId}
      onBack={() => void navigate({ to: '/sprites', search: {} })}
      onOpenRun={(runId) => void navigate({ to: '/runs/$runId', params: { runId } })}
      onOpenMemory={({ scope, view }) =>
        void navigate({
          to: '/memory',
          search: { scope, ...(view === 'pages' ? {} : { view }) },
        })
      }
      onOpenSyncSettings={() =>
        void navigate({ to: '/settings/$section', params: { section: 'connections' } })
      }
    />
  )
}

const agentRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => rootRoute,
  path: '/sprites/$agentId',
  component: AgentView,
})

function AgentDeskView() {
  const api = useApi()
  const { agentId } = agentDeskRoute.useParams()
  const agents = useAgents(api)
  const channels = useChannels(api)
  const phone = useIsMobile()
  const navigate = useNavigate()
  const { from } = agentDeskRoute.useSearch()
  const agent = agents.data?.find((row) => row.id === agentId)
  useEffect(() => {
    if (!phone && agent) {
      const channelId = directMessageChannel(channels.data ?? [], agent.id)
      if (channelId) void navigate({ to: '/c/$channelId', params: { channelId }, search: { panel: 'desk' }, replace: true })
    }
  }, [phone, agent, channels.data, navigate])
  if (!agent) return <p className="phone-hint">That sprite is not one of yours.</p>
  return phone ? <DeskPhone api={api} agent={agent} onClose={() => void navigate({ href: from ?? `/sprites/${agentId}` })} /> : null
}
const agentDeskRoute = createRoute({ getParentRoute: () => rootRoute, path: '/sprites/$agentId/desk', component: AgentDeskView, staticData: { phone: 'pushed' } })

function AgentSectionView({ section }: { section: 'access' | 'memory' | 'work' }) {
  const api = useApi()
  const { agentId, resourceId } = useParams({ strict: false })
  const agents = useAgents(api)
  const phone = useIsMobile()
  const navigate = useNavigate()
  const agent = agents.data?.find((row) => row.id === agentId)
  useEffect(() => {
    if (!phone && agentId) void navigate({ to: '/sprites/$agentId', params: { agentId }, replace: true })
  }, [phone, agentId, navigate])
  if (!phone) return null
  if (!agent) return <p className="phone-hint">{agents.isPending ? 'Reading the sprite…' : 'That sprite is not one of yours.'}</p>
  const openRun = (runId: string) => void navigate({ to: '/runs/$runId', params: { runId }, search: { from: `/sprites/${agent.id}/work` } })
  if (section === 'access') return <AgentAccessPhone key={`${agent.id}:${resourceId ?? 'list'}`} api={api} agent={agent} resourceId={resourceId} />
  const content = section === 'memory' ? <AgentMemory api={api} agent={agent} onOpenMemory={(location) => void navigate({ to: '/memory', search: { ...location, from: `/sprites/${agent.id}/memory` } })} onOpenSyncSettings={() => void navigate({ to: '/settings/$section', params: { section: 'connections' } })} onOpenRun={openRun} /> : <AgentWork api={api} agentId={agent.id} agentName={agent.name} onOpenRun={openRun} />
  return <><NavBar back={{ label: agent.name, onBack: () => void navigate({ to: '/sprites/$agentId', params: { agentId: agent.id } }) }} /><div className="phone-content"><h1 className="phone-heading">{section.charAt(0).toUpperCase() + section.slice(1)}</h1>{content}</div></>
}
const agentAccessRoute = createRoute({ getParentRoute: () => rootRoute, path: '/sprites/$agentId/access', component: () => <AgentSectionView section="access" />, staticData: { phone: 'pushed' } })
const agentAccessResourceRoute = createRoute({ getParentRoute: () => rootRoute, path: '/sprites/$agentId/access/$resourceId', component: () => <AgentSectionView section="access" />, staticData: { phone: 'pushed' } })
const agentMemoryRoute = createRoute({ getParentRoute: () => rootRoute, path: '/sprites/$agentId/memory', component: () => <AgentSectionView section="memory" />, staticData: { phone: 'pushed' } })
const agentWorkRoute = createRoute({ getParentRoute: () => rootRoute, path: '/sprites/$agentId/work', component: () => <AgentSectionView section="work" />, staticData: { phone: 'pushed' } })

export interface MemorySearch {
  scope?: string
  path?: string
  view?: MemoryView
}

/** Memory: the pages of one scope and the page the daemon
 *  holds. The URL holds the scope, the page and the view, so the Agent
 *  profile links a scope. */
function MemoryPlaceView() {
  const api = useApi()
  const navigate = useNavigate()
  const { scope, path, view, from } = memoryRoute.useSearch()
  const phone = useIsMobile()
  return (
    <>{phone && <NavBar back={{ label: path ? 'Memory' : from ? 'Sprite' : 'You', onBack: () => path ? void navigate({ to: '/memory', search: { scope, view, from } }) : void navigate({ href: from ?? '/you' }) }} />}<MemoryPage
      api={api}
      scope={scope}
      path={path}
      view={view ?? 'pages'}
      onChange={(next) =>
        void navigate({
          to: '/memory',
          search: {
            scope: next.scope,
            from,
            ...(next.path === undefined ? {} : { path: next.path }),
            ...(next.view === 'pages' ? {} : { view: next.view }),
          },
        })
      }
      onOpenChannel={(channelId) =>
        void navigate({ to: '/c/$channelId', params: { channelId }, search: {} })
      }
      onOpenRun={(runId) => void navigate({ to: '/runs/$runId', params: { runId } })}
    /></>
  )
}

const memoryRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => rootRoute,
  path: '/memory',
  component: MemoryPlaceView,
  validateSearch: (search: Record<string, unknown>): MemorySearch => ({
    ...(typeof search.scope === 'string' ? { scope: search.scope } : {}),
    ...(typeof search.path === 'string' ? { path: search.path } : {}),
    ...(search.view === 'changes' || search.view === 'procedures' ? { view: search.view } : {}),
  }),
})

function AutomationsView() {
  const api = useApi()
  const navigate = useNavigate()
  const { rule } = automationsRoute.useSearch()
  const phone = useIsMobile()
  return (
    <Automations
      api={api}
      selection={phone ? rule ?? null : undefined}
      onSelect={(next) => void navigate({ to: '/automations', search: { rule: next ?? undefined } })}
      onClose={() => void navigate({ to: phone ? '/you' : '/' })}
      onOpenChannel={(channelId) =>
        void navigate({ to: '/c/$channelId', params: { channelId }, search: {} })
      }
    />
  )
}

const automationsRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => rootRoute,
  path: '/automations',
  component: AutomationsView,
  validateSearch: (search: Record<string, unknown>): { rule?: { kind: 'schedule' | 'subscription'; id: string } } => { const rule = search.rule; if (rule && typeof rule === 'object' && 'kind' in rule && 'id' in rule && (rule.kind === 'schedule' || rule.kind === 'subscription') && typeof rule.id === 'string') return { rule: { kind: rule.kind, id: rule.id } }; return {} },
})

function SoftwareView() {
  const api = useApi()
  const navigate = useNavigate()
  const { package: packageName } = softwareRoute.useSearch()
  const phone = useIsMobile()
  return (
    <Software
      api={api}
      selection={phone ? packageName ?? null : undefined}
      onSelect={(name) => void navigate({ to: '/software', search: { package: name ?? undefined } })}
      onClose={() => void navigate({ to: phone ? '/you' : '/' })}
      onOpenChannel={(channelId) =>
        void navigate({ to: '/c/$channelId', params: { channelId }, search: {} })
      }
    />
  )
}

const softwareRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => rootRoute,
  path: '/software',
  component: SoftwareView,
  validateSearch: (search: Record<string, unknown>): { package?: string } => typeof search.package === 'string' ? { package: search.package } : {},
})

/** Settings holds the workspace and the system, in three groups.
 *  An address that names no section lands on the sprites board. */
function SettingsView() {
  const api = useApi()
  const { section } = settingsRoute.useParams()
  const navigate = useNavigate()
  const known = isSettingsSection(section) ? section : null
  const phone = useIsMobile()
  const isAdministrator = useIsAdministrator(api)
  // A member who types the address of an administrator section lands on
  // the first section they may open.
  const forbidden = known !== null && isAdministratorSection(known) && (!isAdministrator || phone)

  useEffect(() => {
    if (known === null) {
      void navigate({ to: '/sprites', search: {}, replace: true })
      return
    }
    if (forbidden && phone) { void navigate({ to: '/settings', replace: true }); return }
    if (forbidden) {
      void navigate({
        to: '/settings/$section',
        params: { section: visibleSettingsSections(false)[0].value },
        replace: true,
      })
    }
  }, [forbidden, known, navigate, phone])

  if (known === null || forbidden) return null
  return (
    <SettingsPanel
      api={api}
      section={known}
      isAdministrator={isAdministrator}
      onSelectSection={(next) =>
        void navigate({ to: '/settings/$section', params: { section: next } })
      }
      onOpenConnection={(connectionId) =>
        void navigate({
          to: '/settings/connections/$connectionId',
          params: { connectionId },
        })
      }
    />
  )
}

/** One connection is one address. It draws in the settings
 *  frame, under the Connections section of the list. */
function ConnectionView() {
  const api = useApi()
  const { connectionId } = connectionRoute.useParams()
  const navigate = useNavigate()
  const isAdministrator = useIsAdministrator(api)
  const openConnections = () =>
    void navigate({ to: '/settings/$section', params: { section: 'connections' } })
  return (
    <SettingsShell
      section="connections"
      isAdministrator={isAdministrator}
      onSelectSection={(next) =>
        void navigate({ to: '/settings/$section', params: { section: next } })
      }
    >
      <ConnectionPage api={api} connectionId={connectionId} onBack={openConnections} />
    </SettingsShell>
  )
}

const connectionRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => rootRoute,
  path: '/settings/connections/$connectionId',
  component: ConnectionView,
})

const settingsRoute = createRoute({
  staticData: { phone: 'pushed' },
  getParentRoute: () => rootRoute,
  path: '/settings/$section',
  component: SettingsView,
})

/** `/settings` holds no view of its own: it opens the first section
 *  that every person sees. */
function SettingsIndexView() {
  const api = useApi()
  const phone = useIsMobile()
  const navigate = useNavigate()
  useEffect(() => { if (!phone) void navigate({ to: '/settings/$section', params: { section: 'connections' }, replace: true }) }, [phone, navigate])
  return phone ? <SettingsPhone api={api} /> : null
}
const settingsIndexRoute = createRoute({ getParentRoute: () => rootRoute, path: '/settings', component: SettingsIndexView, staticData: { phone: 'pushed' } })

function KeypadView() {
  const api = useApi()
  const list = useTrustList(api)
  const navigate = useNavigate()
  const phone = useIsMobile()
  useEffect(() => {
    if (!phone) void navigate({ to: '/settings/$section', params: { section: 'trusted-contacts' }, replace: true })
  }, [phone, navigate])
  if (!phone) return null
  return <><NavBar back={{ label: 'Trusted contacts', onBack: () => void navigate({ to: '/settings/$section', params: { section: 'trusted-contacts' } }) }} /><div className="phone-content"><h1 className="phone-heading">Keypad code</h1>{list.data && <KeypadCodeRow api={api} keypad={list.data.keypad_code} />}{list.isError && <p role="alert" className="phone-hint">Could not read the keypad code.</p>}</div></>
}
const keypadRoute = createRoute({ getParentRoute: () => rootRoute, path: '/settings/trusted-contacts/keypad', component: KeypadView, staticData: { phone: 'pushed' } })

function ConversationsPhoneView() {
  const api = useApi()
  const phone = useIsMobile()
  const navigate = useNavigate()
  useEffect(() => { if (!phone) void navigate({ to: '/c', replace: true }) }, [phone, navigate])
  return phone ? <ConversationsPhone api={api} /> : null
}
const phoneConversationsRoute = createRoute({ getParentRoute: () => rootRoute, path: '/conversations', component: ConversationsPhoneView, staticData: { phone: 'tab' }, validateSearch: (search: Record<string, unknown>): { new?: 'group' } => search.new === 'group' ? { new: 'group' } : {} })
function YouView() {
  const api = useApi()
  const phone = useIsMobile()
  const navigate = useNavigate()
  useEffect(() => { if (!phone) void navigate({ to: '/settings', replace: true }) }, [phone, navigate])
  return phone ? <You api={api} /> : null
}
const youRoute = createRoute({ getParentRoute: () => rootRoute, path: '/you', component: YouView, staticData: { phone: 'tab' } })
function CallView() {
  const api = useApi()
  const { callId } = callRoute.useParams()
  const navigate = useNavigate()
  const phone = useIsMobile()
  return phone ? <CallPhone api={api} callId={callId} /> : <CallInspector api={api} callId={callId} onClose={() => void navigate({ to: '/' })} />
}
const callRoute = createRoute({ getParentRoute: () => rootRoute, path: '/calls/$callId', component: CallView, staticData: { phone: 'pushed' } })

/** The start route of a brokered Google authorization on the daemon. */
const GOOGLE_START_ROUTE = '/api/v1/connections/google/start'

export interface GoogleStartSearch {
  state?: string
}

/** The way back to a Google authorization after a sign-in. The start
 *  route of the daemon sends a browser with no session here. The app
 *  shows its sign-in page at this address, as at every address, and
 *  once the Person is signed in, this view sends the browser back to the
 *  start route, which goes on to Google. */
function GoogleStartView() {
  const { state } = googleStartRoute.useSearch()
  useEffect(() => {
    if (state !== undefined) {
      openDocument(`${GOOGLE_START_ROUTE}?state=${encodeURIComponent(state)}`)
    }
  }, [state])
  return (
    <p className="settings-hint" role="status">
      {state === undefined ? 'This address names no Google sign-in.' : 'Opening Google…'}
    </p>
  )
}

const googleStartRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/connections/google/start',
  component: GoogleStartView,
  validateSearch: (search: Record<string, unknown>): GoogleStartSearch =>
    typeof search.state === 'string' && search.state !== '' ? { state: search.state } : {},
})

const routeTree = rootRoute.addChildren([
  indexRoute,
  phoneConversationsRoute,
  youRoute,
  callRoute,
  spritesRoute,
  conversationsRoute.addChildren([
    conversationsIndexRoute,
    channelRoute.addChildren([threadRoute]),
  ]),
  runsRoute.addChildren([runsIndexRoute, runRoute]),
  codingRoute.addChildren([codingSessionRoute]),
  agentRoute,
  agentDeskRoute,
  agentAccessRoute,
  agentAccessResourceRoute,
  agentMemoryRoute,
  agentWorkRoute,
  memoryRoute,
  automationsRoute,
  softwareRoute,
  settingsIndexRoute,
  keypadRoute,
  settingsRoute,
  connectionRoute,
  googleStartRoute,
])

export function createAppRouter(context: RouterContext, history?: RouterHistory) {
  return createRouter({ routeTree, context, history })
}

declare module '@tanstack/react-router' {
  interface StaticDataRouteOption { phone?: 'tab' | 'pushed' | 'sheet' }
  interface Register {
    router: ReturnType<typeof createAppRouter>
  }
}
