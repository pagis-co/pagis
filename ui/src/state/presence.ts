// Presence in the sidebar: who is working, who needs you, who is
// on a call, who is idle. Nothing new comes from the daemon — the
// state is derived from the runs and the calls the firehose already
// publishes, so a ring turns without a refetch.
//
// The store holds only what is live: the unfinished runs, the agents on
// a call, and the channels with a message the reader has not seen.

import { useQuery } from '@tanstack/react-query'
import { useEffect } from 'react'
import { create } from 'zustand'

import type { ApiClient, EventRow, RunDto } from '../api/client'
import type { Presence } from '../primitives'
import { runsKey } from '../queries'

/** The run states that are not terminal. The list API takes a set of
 *  states, so the seed asks for all of them in one call. */
export const LIVE_RUN_STATES = [
  'queued',
  'running',
  'reflecting',
  'waiting_for_user',
  'waiting_for_approval',
] as const

/** The caption a trigger gives the row. The daemon names the trigger,
 *  the sidebar says it in one line. */
function captionForTrigger(triggerKind: string): string {
  switch (triggerKind) {
    case 'message':
      return 'Working on your message'
    case 'schedule':
      return 'Working on a schedule'
    case 'event':
      return 'Working on an event'
    default:
      return 'Working'
  }
}

function runCaption(state: string, triggerKind: string): string {
  return state === 'reflecting' ? 'Updating memory' : captionForTrigger(triggerKind)
}

/** One unfinished run, as much of it as a ring and a caption need. */
export interface LiveRun {
  runId: string
  agentId: string
  channelId: string | null
  /** The conversation the run's delegation chain owes an answer to.
   *  The Desk Panel reads it (ADR-0022). */
  originChannelId: string | null
  state: string
  caption: string
}

export interface PresenceState {
  /** The unfinished runs, keyed by run id. A terminal run leaves. */
  runs: Record<string, LiveRun>
  /** The call each agent is on now, by agent id. */
  onCall: Record<string, string>
  /** The channels with a settled agent message the reader missed. */
  unread: Record<string, true>
  /** The open channel: its messages are read as they land. */
  selectedChannelId: string | null
  /** The first runs list has landed; a later list never fights the
   *  frames, which are ahead of it. */
  seeded: boolean
  /** Take the unfinished runs from the list API, once. */
  seed: (runs: readonly RunDto[]) => void
  /** Fold one firehose frame. Every other frame is ignored. */
  applyFrame: (type: string, event: EventRow) => void
  /** The reader opened a channel: it has nothing unread. */
  select: (channelId: string | null) => void
}

function isTerminal(state: string): boolean {
  return state === 'completed' || state === 'failed' || state === 'canceled'
}

export const usePresence = create<PresenceState>((set) => ({
  runs: {},
  onCall: {},
  unread: {},
  selectedChannelId: null,
  seeded: false,
  seed: (runs) =>
    set((state) => {
      if (state.seeded) return state
      const held: Record<string, LiveRun> = { ...state.runs }
      for (const run of runs) {
        if (isTerminal(run.state) || held[run.id] !== undefined) continue
        held[run.id] = {
          runId: run.id,
          agentId: run.agent_id,
          channelId: run.channel_id ?? null,
          originChannelId: run.origin_channel_id ?? null,
          state: run.state,
          caption: runCaption(run.state, run.trigger_kind),
        }
      }
      return { runs: held, seeded: true }
    }),
  applyFrame: (type, event) =>
    set((state) => {
      if (type === 'run.created' || type === 'run.state_changed') {
        const runId = event.run_id
        const agentId = event.agent_id
        if (runId == null || agentId == null) return state
        const payload = event.payload as {
          to?: string
          trigger_kind?: string
          origin_channel_id?: string | null
        }
        const to = type === 'run.created' ? 'queued' : (payload.to ?? '')
        if (isTerminal(to)) {
          if (state.runs[runId] === undefined) return state
          const { [runId]: _gone, ...rest } = state.runs
          return { runs: rest }
        }
        const held = state.runs[runId]
        const run: LiveRun = {
          runId,
          agentId,
          channelId: event.channel_id ?? null,
          // Only `run.created` carries the origin; a state change keeps
          // the one the store already holds.
          originChannelId: payload.origin_channel_id ?? held?.originChannelId ?? null,
          state: to,
          caption:
            to === 'reflecting'
              ? 'Updating memory'
              : payload.trigger_kind === undefined
                ? (held?.caption ?? 'Working')
                : captionForTrigger(payload.trigger_kind),
        }
        return { runs: { ...state.runs, [runId]: run } }
      }
      if (type.startsWith('call.')) {
        const agentId = event.agent_id
        if (agentId == null) return state
        if (type === 'call.ended') {
          if (state.onCall[agentId] === undefined) return state
          const { [agentId]: _gone, ...rest } = state.onCall
          return { onCall: rest }
        }
        if (type === 'call.placed' || type === 'call.answered') {
          const callId = (event.payload as { call_id?: string }).call_id
          if (callId == null) return state
          return { onCall: { ...state.onCall, [agentId]: callId } }
        }
        return state
      }
      if (type === 'message.completed') {
        const channelId = event.channel_id
        const payload = event.payload as { author_kind?: string }
        if (
          channelId == null ||
          payload.author_kind !== 'agent' ||
          channelId === state.selectedChannelId
        ) {
          return state
        }
        return { unread: { ...state.unread, [channelId]: true } }
      }
      return state
    }),
  select: (channelId) =>
    set((state) => {
      if (channelId === null) return { selectedChannelId: null }
      const { [channelId]: _read, ...rest } = state.unread
      return { selectedChannelId: channelId, unread: rest }
    }),
}))

/** The work a ring stands for: the Agent prepares a reply or does its
 *  main work in a conversation. A Run that reflects settles memory
 *  after its work ends (ADR-0002), and an arrival Run reflects in the
 *  background with no Channel at all (ADR-0011). The user waits on
 *  neither, so neither turns a ring. */
function isVisibleWork(run: LiveRun): boolean {
  return run.channelId !== null && (run.state === 'queued' || run.state === 'running')
}

/** A Run that asks the user for something, wherever it works. */
function needsUser(run: LiveRun): boolean {
  return run.state === 'waiting_for_user' || run.state === 'waiting_for_approval'
}

/** The presence of one agent. A call wins over a wait, and a wait over
 *  work, because it is the state that most needs the reader. */
export function agentPresence(state: PresenceState, agentId: string): Presence {
  if (state.onCall[agentId] !== undefined) return 'oncall'
  let working = false
  for (const run of Object.values(state.runs)) {
    if (run.agentId !== agentId) continue
    if (needsUser(run)) return 'waiting'
    if (isVisibleWork(run)) working = true
  }
  return working ? 'working' : 'idle'
}

/** The presence one Channel shows for one Agent: the work that Agent
 *  does in this Channel alone. A shared Channel lights up while the
 *  Agent writes in it, not while the Agent works somewhere else. The
 *  Agent's own Channel with the user reads `agentPresence` instead,
 *  because it is where the user follows that Agent. */
export function channelPresence(
  state: PresenceState,
  channelId: string,
  agentId: string,
): Presence {
  let working = false
  for (const run of Object.values(state.runs)) {
    if (run.agentId !== agentId || run.channelId !== channelId) continue
    if (needsUser(run)) return 'waiting'
    if (isVisibleWork(run)) working = true
  }
  return working ? 'working' : 'idle'
}

/** The Call one agent is on, or null. The Thread header joins it. */
export function liveCallOf(state: PresenceState, agentId: string): string | null {
  return state.onCall[agentId] ?? null
}

/** The one line under the title: the newest live run in the channel. */
export function channelCaption(
  state: PresenceState,
  channelId: string,
): string | null {
  let caption: string | null = null
  for (const run of Object.values(state.runs)) {
    if (run.channelId === channelId) caption = run.caption
  }
  return caption
}

export function isUnread(state: PresenceState, channelId: string): boolean {
  return state.unread[channelId] === true
}

/** Seed the store from the runs list, so the rings are right before the
 *  first frame. The list API filters on a set of states, so one
 *  request gets every live state (`LIVE_RUN_STATES`). */
export function usePresenceSeed(api: ApiClient): void {
  const state = LIVE_RUN_STATES.join(',')
  const { data, isFetching } = useQuery({
    queryKey: runsKey('', '', state),
    queryFn: () =>
      api
        .GET('/api/v1/runs', { params: { query: { state } } })
        .then(({ data }) => data?.items ?? []),
  })
  const seed = usePresence((store) => store.seed)
  const seeded = usePresence((store) => store.seeded)
  useEffect(() => {
    if (data !== undefined && !isFetching && !seeded) seed(data)
  }, [data, seed, seeded, isFetching])
}
