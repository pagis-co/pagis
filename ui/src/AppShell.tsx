// The app shell: the workspace navigation, the pane the route fills,
// and the inspector slot. Every view is a URL, so the shell
// reads the route rather than a boolean per destination.

import { createAvatarReactions } from './avatars/reactions'
import { useAvatarReaction } from './avatars/liveMotion'
import { AvatarRoster } from './avatars/AvatarRoster'
import { useQueryClient } from '@tanstack/react-query'
import {
  Outlet,
  useLocation,
  useNavigate,
  useParams,
  useRouteContext,
  useSearch,
} from '@tanstack/react-router'
import { WifiOff } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import type { DeltaFrame, EventRow, ProgressFrame } from './api/client'
import { CallInspector } from './components/CallInspector'
import { DeskPanel } from './components/desk/DeskPanel'
import { MailInspector } from './components/MailInspector'
import {
  CommandPalette,
  applyStoredTheme,
} from './components/CommandPalette'
import { Onboarding } from './components/onboarding/Onboarding'
import { directMessageChannel } from './components/AskAnAgent'
import type { DeskSurface } from './components/desk/desks'
import { chiefOfStaff } from './components/sidebar/conversations'
import { Sidebar } from './components/sidebar/Sidebar'
import type { PlaceId } from './components/sidebar/places'
import { ThreadPane } from './components/ThreadPane'
import {
  agentsKey,
  workspaceKey,
  callKey,
  codingSessionEventsKey,
  codingSessionKey,
  needsYouKey,
  pendingRequestsKey,
  pluginKey,
  pluginsKey,
  requestKey,
  schedulesKey,
  softwareKey,
  subscriptionsKey,
  channelsKey,
  connectionsKey,
  computerKey,
  credentialsKey,
  agentMailboxKey,
  mailboxesKey,
  phoneNumbersKey,
  grantsKey,
  memoryFeedKey,
  modelAliasesKey,
  runStepsKey,
  runTranscriptKey,
  screenPreviewKey,
  threadKey,
  timelineKey,
  trustListKey,
  useAgents,
  useChannels,
  useOnboarding,
  useSignOut,
  useWorkspace,
} from './queries'
import { useNotificationSync } from './push/useNotificationSync'
import { useShellNavigation } from './mobileShell'
import { createSpeaker } from './speech'
import {
  useCallInspector,
  useMailInspector,
  useCallTranscripts,
  useConnection,
  useLiveStreams,
  useRunProgress,
  useSpeaking,
  useTakeoverCountdowns,
  useWidgetTeardowns,
} from './state/stores'
import { cueForFrame, playCue } from './state/sound'
import { useIsCompact, useIsMobile } from './state/useIsMobile'
import { usePresence, usePresenceSeed } from './state/presence'
import { threadScope } from './timeline'
import { watchActivity } from './ws/activity'
import { PagisSocket, type ServerFrame } from './ws/socket'
import { PhoneShell } from './components/phone/PhoneShell'

import './AppShell.css'

function wsUrl(): string {
  const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return `${scheme}://${window.location.host}/api/v1/ws`
}

function ConnectionBanner() {
  const status = useConnection((state) => state.status)
  if (status === 'online') return null
  return (
    <div className="connection-banner" role="status">
      <WifiOff size={14} aria-hidden />
      {status === 'connecting' ? 'Connecting to Pagis…' : 'Connection lost. Reconnecting…'}
    </div>
  )
}

export function AppShell() {
  const { api } = useRouteContext({ from: '__root__' })
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const location = useLocation()
  // The shell reads the route loosely: a channel, a thread and the
  // panel are each present only under the routes that carry them.
  const params = useParams({ strict: false }) as {
    channelId?: string
    messageId?: string
  }
  const search = useSearch({ strict: false }) as { panel?: 'desk' }
  const selectedId = params.channelId ?? null
  const threadRootId = params.messageId ?? null
  const onHome = location.pathname === '/'
  // The loose read sees the raw address, so only `desk` opens the panel,
  // and only on the two views that have one.
  const panel =
    (selectedId !== null || onHome) && search.panel === 'desk' ? search.panel : undefined
  const onboarding = useOnboarding(api)
  const signOut = useSignOut(api)
  const isMobile = useIsMobile()
  const isCompact = useIsCompact()
  // On a wide screen the Desk Panel is derived from the route, not
  // toggled (ADR-0022): it owns the slot on Home and in the Chief of
  // Staff's direct channel. At 1100px and below the slot takes room
  // from the conversation, so the person opens the panel there.
  const agents = useAgents(api)
  const workspaceSetting = useWorkspace(api)
  const channels = useChannels(api)
  const chief = chiefOfStaff(
    agents.data ?? [],
    workspaceSetting.data?.chief_of_staff_agent_id,
  )
  const chiefChannelId =
    chief === null ? null : directMessageChannel(channels.data ?? [], chief.id)
  // The two surfaces that own the panel, and the address that opens it
  // anywhere else.
  const routeOwnsDesk =
    onHome || (selectedId !== null && selectedId === chiefChannelId)
  const deskSurface: DeskSurface | null =
    panel !== undefined || (routeOwnsDesk && !isCompact) ? (onHome ? 'home' : 'channel') : null
  // Only the address-bar tenant is a sheet the user closes; the two
  // surfaces that own the panel keep it beside the page.
  // Presence is right before the first frame.
  usePresenceSeed(api)
  useNotificationSync(api)
  useShellNavigation()
  // The call inspector: transient, and open across navigation.
  const callId = useCallInspector((state) => state.callId)
  const closeCall = useCallInspector((state) => state.close)
  // The mail inspector: transient in the same slot.
  const mail = useMailInspector((state) => state.mail)
  const closeMail = useMailInspector((state) => state.close)

  useEffect(() => {
    if (isMobile && callId) {
      void navigate({ to: '/calls/$callId', params: { callId } })
      closeCall()
    }
  }, [isMobile, callId, closeCall, navigate])

  const goToChannel = (channelId: string, next?: 'desk') =>
    void navigate({
      to: '/c/$channelId',
      params: { channelId },
      search: next === undefined ? {} : { panel: next },
    })

  // The command palette. It names a destination as a path, so
  // the shell is the one that navigates.
  const [paletteOpen, setPaletteOpen] = useState(false)
  const goToPath = useCallback(
    (path: string) => {
      // The router types `to` as the union of the route paths; a path
      // the palette builds is only known at run time.
      void navigate({ to: path as never })
    },
    [navigate],
  )

  useEffect(() => {
    applyStoredTheme()
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setPaletteOpen((open) => !open)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  // One sidebar, one behaviour: every place replaces the
  // main pane and none of them opens the inspector.
  const goToPlace = (place: PlaceId) => {
    if (place === 'home') return void navigate({ to: '/' })
    if (place === 'sprites') return void navigate({ to: '/sprites' })
    if (place === 'memory') return void navigate({ to: '/memory', search: {} })
    if (place === 'automations') return void navigate({ to: '/automations' })
    if (place === 'software') return void navigate({ to: '/software' })
    return void navigate({ to: '/settings/$section', params: { section: 'connections' } })
  }

  // The WS firehose drives cache invalidation; `resync` (gap not
  // covered by the daemon's replay ring) drops every cache. Delta
  // frames feed the live-stream buffers, and `progress.state` frames
  // the derived run lines.
  const socketRef = useRef<PagisSocket | null>(null)
  // Spoken replies: a settled Agent message in a scope that
  // speaks is read aloud, one markdown block at a time, in order.
  const speaker = useMemo(
    () =>
      createSpeaker((channelId, messageId) =>
        api
          .GET('/api/v1/channels/{channel_id}/messages/{message_id}', {
            params: { path: { channel_id: channelId, message_id: messageId } },
          })
          .then(({ data }) => {
            if (data === undefined) throw new Error('message not found')
            return data
          }),
      ),
    [api],
  )
  useEffect(() => {
    const reactions = createAvatarReactions()
    const socket = new PagisSocket({
      url: wsUrl(),
      handlers: {
        onEvent: (frame: ServerFrame) => {
          const event = frame.payload as EventRow
          const presenceBefore = usePresence.getState()
          const reaction = reactions.accept(event, { replay: frame.replay === true,
            channelId: document.hidden ? null : presenceBefore.selectedChannelId,
            previousRunState: event.run_id ? presenceBefore.runs[event.run_id]?.state : undefined })
          if (reaction && useConnection.getState().status === 'online') useAvatarReaction.setState({ reaction: { ...reaction, until: Date.now() + reaction.seconds * 1000 } })
          // Sidebar presence: the rings, the captions and the
          // unread dots derive from the run, call and message frames.
          usePresence.getState().applyFrame(frame.type, event)
          // The sound cues. The setting gates them, so a shell
          // with sound off makes no audio at all.
          const cue = cueForFrame(
            frame.type,
            (event.payload ?? {}) as { kind?: string; to?: string },
          )
          if (cue !== null) playCue(cue)
          if (frame.type === 'channel.created') {
            void queryClient.invalidateQueries({ queryKey: channelsKey })
          }
          // Remove retained chat projections before access or source state is rechecked.
          if (['knowledge.changed', 'grant.changed', 'grant.revoked', 'connection.changed',
            'connection.deleted', 'agent.updated', 'agent.archived'].includes(frame.type)) {
            void queryClient.resetQueries({ queryKey: ['timeline'] })
            void queryClient.resetQueries({ queryKey: ['thread'] })
          }
          // A roster write refreshes the agent list.
          if (
            frame.type === 'agent.created' ||
            frame.type === 'agent.updated' ||
            frame.type === 'agent.archived'
          ) {
            void queryClient.invalidateQueries({ queryKey: agentsKey })
            void queryClient.resetQueries({ queryKey: ['account-sync'] })
          }
          // The Chief of Staff moved, by the user or by an archive
          // (ADR-0022).
          if (frame.type === 'workspace.updated') {
            void queryClient.invalidateQueries({ queryKey: workspaceKey })
          }
          // A grant write refreshes the settings page.
          if (frame.type === 'grant.changed' || frame.type === 'grant.revoked') {
            void queryClient.invalidateQueries({ queryKey: grantsKey })
            void queryClient.resetQueries({ queryKey: ['memory-file'] })
            void queryClient.resetQueries({ queryKey: ['account-sync'] })
          }
          if (frame.type === 'knowledge.changed') {
            void queryClient.resetQueries({ queryKey: ['account-sync'] })
            void queryClient.resetQueries({ queryKey: ['memory-file'] })
          }
          // A durable rule write, or a Wake-up moving, refreshes the
          // Automations destination.
          if (
            frame.type.startsWith('schedule.') ||
            frame.type.startsWith('wakeup.')
          ) {
            void queryClient.invalidateQueries({ queryKey: schedulesKey })
          }
          if (
            frame.type.startsWith('event_subscription.') ||
            frame.type.startsWith('wakeup.')
          ) {
            void queryClient.invalidateQueries({ queryKey: subscriptionsKey })
          }
          // A published package or a Contribution that moved refreshes
          // the Software destination.
          if (
            frame.type === 'software.published' ||
            frame.type === 'contribution.updated'
          ) {
            void queryClient.invalidateQueries({ queryKey: softwareKey })
          }
          // An install, an update, a state change or a changed tool
          // list refreshes the Plugins tab.
          if (frame.type.startsWith('plugin.')) {
            const payload = event.payload as { plugin_id?: string }
            void queryClient.invalidateQueries({ queryKey: pluginsKey })
            if (payload.plugin_id != null) {
              void queryClient.invalidateQueries({
                queryKey: pluginKey(payload.plugin_id),
              })
            }
          }
          // The daemon derives the Needs-You Queue, and tells each client
          // when an item enters or leaves it (ADR-0030). Home and the
          // sidebar count read the queue again.
          if (frame.type === 'needs_you.added' || frame.type === 'needs_you.removed') {
            void queryClient.invalidateQueries({ queryKey: needsYouKey })
          }
          if (frame.type.startsWith('request.')) {
            void queryClient.invalidateQueries({ queryKey: pendingRequestsKey })
          }
          if (frame.type.startsWith('run.')) {
            void queryClient.invalidateQueries({ queryKey: ['runs'] })
          }
          if (event.run_id != null) {
            void queryClient.invalidateQueries({
              queryKey: runTranscriptKey(event.run_id),
            })
            // The work record and the Working row read the steps,
            // so a tool event moves them too.
            void queryClient.invalidateQueries({
              queryKey: runStepsKey(event.run_id),
            })
          }
          if (frame.type.startsWith('model_alias.')) {
            void queryClient.invalidateQueries({ queryKey: modelAliasesKey })
          }
          if (frame.type === 'connection.deleted' || frame.type === 'connection.changed') {
            void queryClient.invalidateQueries({ queryKey: connectionsKey })
            void queryClient.resetQueries({ queryKey: ['account-sync'] })
            void queryClient.resetQueries({ queryKey: ['memory-file'] })
            void queryClient.invalidateQueries({ queryKey: grantsKey })
          }
          // The Call surfaces. The record stays the source of
          // truth, so a call event that changes it refetches it; a
          // transcript line is buffered until the next read carries it.
          if (frame.type.startsWith('call.')) {
            // A wrong keypad code moves the failed-attempt count of the
            // Workspace, and a correct one clears it before the call is
            // answered or while it runs (ADR-0021). The Keypad Code card
            // reads that count.
            if (
              frame.type === 'call.keypad_failed' ||
              frame.type === 'call.answered' ||
              frame.type === 'call.tier_changed'
            ) {
              void queryClient.invalidateQueries({ queryKey: trustListKey })
            }
            const payload = event.payload as {
              call_id?: string
              at?: number
              speaker?: string
              text?: string
            }
            if (payload.call_id != null) {
              if (
                frame.type === 'call.transcript' &&
                payload.at != null &&
                payload.speaker != null &&
                payload.text != null
              ) {
                useCallTranscripts.getState().append(payload.call_id, {
                  at: payload.at,
                  speaker: payload.speaker,
                  text: payload.text,
                })
              } else {
                void queryClient.invalidateQueries({
                  queryKey: callKey(payload.call_id),
                })
              }
            }
          }
          // A Coding Session reports each write of its record and of its
          // transcript, with no text (ADR-0033). The session page reads
          // both again.
          if (frame.type.startsWith('coding_session.')) {
            const sessionId = (event.payload as { coding_session_id?: string })
              .coding_session_id
            if (sessionId != null) {
              void queryClient.invalidateQueries({ queryKey: codingSessionKey(sessionId) })
              void queryClient.invalidateQueries({
                queryKey: codingSessionEventsKey(sessionId),
              })
            }
          }
          // A number bought, assigned, unassigned or released refreshes
          // the Agent's settings page.
          if (frame.type.startsWith('phone_number.')) {
            void queryClient.invalidateQueries({ queryKey: phoneNumbersKey })
          }
          // A Run that ended dropped its Widget views
          // (ADR-0016). Each frame hears the extension's
          // `ui/resource-teardown` before it stops.
          if (frame.type === 'widget.torn_down') {
            const ids = (event.payload as { tool_call_ids?: unknown })
              .tool_call_ids
            if (Array.isArray(ids)) {
              useWidgetTeardowns
                .getState()
                .tearDown(ids.filter((id): id is string => typeof id === 'string'))
            }
          }
          // A mailbox made, proved, reset, put to sleep or deleted
          // moves the Agent's card and the Connections page.
          if (frame.type.startsWith('agent_mailbox.')) {
            const agentId = (frame.payload as { agent_id?: string } | undefined)
              ?.agent_id
            if (agentId !== undefined) {
              void queryClient.invalidateQueries({
                queryKey: agentMailboxKey(agentId),
              })
            }
            void queryClient.invalidateQueries({ queryKey: mailboxesKey })
            void queryClient.invalidateQueries({ queryKey: agentsKey })
          }
          if (frame.type === 'credential.deleted') {
            void queryClient.invalidateQueries({ queryKey: credentialsKey })
            void queryClient.invalidateQueries({ queryKey: grantsKey })
          }
          // A memory change reaches the learned lines live.
          if (
            frame.type === 'memory.committed' ||
            frame.type === 'memory.reverted'
          ) {
            void queryClient.invalidateQueries({ queryKey: memoryFeedKey })
            void queryClient.invalidateQueries({ queryKey: ['memory-pages'] })
            void queryClient.resetQueries({ queryKey: ['memory-file'] })
          }
          // A computer transition refreshes its tile and preview.
          if (frame.type === 'computer.state_changed' && event.agent_id != null) {
            void queryClient.invalidateQueries({
              queryKey: computerKey(event.agent_id),
            })
            void queryClient.invalidateQueries({
              queryKey: screenPreviewKey(event.agent_id),
            })
          }
          // A change of the exit in use (ADR-0029) refreshes the tile,
          // which names the exit.
          if (frame.type === 'computer.exit_changed' && event.agent_id != null) {
            void queryClient.invalidateQueries({
              queryKey: computerKey(event.agent_id),
            })
          }
          // Takeover state and the handback countdown toast.
          if (frame.type.startsWith('screen.') && event.agent_id != null) {
            void queryClient.invalidateQueries({
              queryKey: computerKey(event.agent_id),
            })
            if (frame.type === 'screen.handback_countdown') {
              const seconds =
                (event.payload as { seconds?: number }).seconds ?? 15
              useTakeoverCountdowns.getState().start(event.agent_id, seconds)
            }
            if (
              frame.type === 'screen.handback_countdown_canceled' ||
              frame.type === 'screen.takeover_ended'
            ) {
              useTakeoverCountdowns.getState().clear(event.agent_id)
            }
          }
          // The decision re-renders every card of the Request.
          // A message in place of a decision settles it the same way.
          if (
            frame.type === 'request.decided' ||
            frame.type === 'request.superseded'
          ) {
            const requestId = (event.payload as { request_id?: string })
              .request_id
            if (requestId != null) {
              void queryClient.invalidateQueries({
                queryKey: requestKey(requestId),
              })
            }
          }
          const settled =
            frame.type === 'message.completed' || frame.type === 'message.failed'
          if (settled && event.channel_id != null) {
            void queryClient.invalidateQueries({ queryKey: channelsKey })
            const payload = event.payload as {
              message_id?: string
              parent_message_id?: string | null
              author_kind?: string
            }
            if (payload.message_id != null) {
              const scope = threadScope(
                event.channel_id,
                payload.parent_message_id,
              )
              useLiveStreams.getState().clear(scope, payload.message_id)
              // A settled progress row holds the terminal line
              // itself, so its live buffer goes.
              useRunProgress.getState().settle(scope, payload.message_id)
              if (
                frame.type === 'message.completed' &&
                payload.author_kind === 'agent' &&
                useSpeaking.getState().byScope[scope] === true
              ) {
                const messageId = payload.message_id
                void speaker
                  .speak(event.channel_id, messageId)
                  .then(() => useSpeaking.getState().markSpoken(messageId))
                  .catch(() => undefined)
              }
            }
            // A reply also changes its root's rollup, so the timeline
            // refetches for thread messages too.
            void queryClient.invalidateQueries({
              queryKey: timelineKey(event.channel_id),
            })
            if (payload.parent_message_id != null) {
              void queryClient.invalidateQueries({
                queryKey: threadKey(event.channel_id, payload.parent_message_id),
              })
            }
          }
        },
        onDelta: (frame: ServerFrame) => {
          useLiveStreams.getState().apply(frame.payload as DeltaFrame)
        },
        onProgress: (frame: ServerFrame) => {
          useRunProgress.getState().apply(frame.payload as ProgressFrame)
        },
        onResync: () => {
          useAvatarReaction.setState({ reaction: null })
          usePresence.setState({ runs: {}, onCall: {}, seeded: false })
          void queryClient.invalidateQueries()
        },
        onStatus: (status) => {
          useConnection.getState().setStatus(status)
          if (status !== 'online') useAvatarReaction.setState({ reaction: null })
        },
        // The Session ended somewhere else. As at a sign-out, every
        // cached answer belonged to it, so every query reads again, and
        // the refused read of the person shows the sign-in page.
        onSignedOut: () => void queryClient.resetQueries(),
      },
    })
    socketRef.current = socket
    socket.start()
    // Input in this visible client holds a new Notification (ADR-0030).
    const stopActivity = watchActivity(() => socket.activity())
    return () => {
      stopActivity()
      socketRef.current = null
      socket.stop()
    }
  }, [queryClient, speaker])

  // The selected channel is the subscribed one: its live streams and
  // run progress (with their mid-run catch-up) arrive as
  // `message.delta` and `progress.state` frames.
  useEffect(() => {
    socketRef.current?.subscribeChannel(selectedId)
    // The open channel is read as it lands.
    usePresence.getState().select(selectedId)
  }, [selectedId])

  // First run: the wizard gates the chat until it completes;
  // finishing lands in the seeded DM with the default assistant.
  if (onboarding.data !== undefined && !onboarding.data.completed) {
    return <AvatarRoster agents={agents.data ?? []}><Onboarding api={api} /></AvatarRoster>
  }

  if (isMobile) return <AvatarRoster agents={agents.data ?? []}><PhoneShell api={api} banner={<ConnectionBanner />} /></AvatarRoster>

  return <AvatarRoster agents={agents.data ?? []}><div className="app">
    <ConnectionBanner />
    <div className="app-body">
      <Sidebar api={api} pathname={location.pathname} selectedId={selectedId}
        onSelectPlace={goToPlace} onSelectChannel={goToChannel}
        onSearch={() => setPaletteOpen(true)} onSignOut={() => signOut.mutate()} />
      <main className="channel-pane"><Outlet /></main>
      {callId !== null ? <aside className="workspace-inspector"><CallInspector api={api} callId={callId} onClose={closeCall} /></aside>
        : mail !== null ? <aside className="workspace-inspector"><MailInspector api={api} mail={mail} onClose={closeMail} /></aside>
        : selectedId !== null && threadRootId !== null ? <ThreadPane api={api} channelId={selectedId} rootId={threadRootId} onClose={() => goToChannel(selectedId)} onOpenDesk={() => goToChannel(selectedId, 'desk')} />
        : deskSurface !== null && <aside className="workspace-inspector"><DeskPanel api={api} surface={deskSurface} channelId={selectedId}
          onOpenAgent={(agentId) => void navigate({ to: '/sprites/$agentId', params: { agentId } })}
          onOpenChannel={(agentId) => { const id = directMessageChannel(channels.data ?? [], agentId); if (id) goToChannel(id) }}
          onNew={() => void navigate({ to: '/sprites', search: { new: '1' } })} /></aside>}
    </div>
    {paletteOpen && <CommandPalette api={api} open onOpenChange={setPaletteOpen} onNavigate={goToPath} />}
  </div></AvatarRoster>
}
