// Ephemeral client state (Zustand): the socket status, the pending
// sends, and the live agent streams. Both are keyed by scope — one
// channel's top level, or one thread (`threadScope`). Server state
// lives in TanStack Query.

import { create } from 'zustand'

import type { DeltaFrame, ProgressFrame } from '../api/client'
import { MAX_LIVE_WIDGETS } from '../blocks/widget'
import type { MailBlockDto } from '../blocks/mail'
import { threadScope, type PendingSend } from '../timeline'
import type { SocketStatus } from '../ws/socket'

interface ConnectionState {
  status: SocketStatus
  setStatus: (status: SocketStatus) => void
}

export const useConnection = create<ConnectionState>((set) => ({
  status: 'connecting',
  setStatus: (status) => set({ status }),
}))

/** One in-flight agent reply, folded from `message.delta` frames. */
export interface LiveStream {
  message_id: string
  run_id: string
  /** The agent writing it; the row's name before the server row lands. */
  agent_id: string
  /** The last folded delta seq; the dedup cursor. */
  seq: number
  text: string
}

const NO_STREAMS: Record<string, LiveStream> = {}

export function selectLiveStreams(scope: string) {
  return (state: LiveState): Record<string, LiveStream> =>
    state.byScope[scope] ?? NO_STREAMS
}

export interface LiveState {
  byScope: Record<string, Record<string, LiveStream>>
  /**
   * Fold one delta frame into its scope (the frame's channel and
   * thread). A catch-up frame replaces the buffer; a live frame appends
   * only when its `seq` directly follows the folded one, so replays
   * after a catch-up never duplicate text.
   */
  apply: (frame: DeltaFrame) => void
  /** The message settled (completed or failed): drop its buffer. */
  clear: (scope: string, messageId: string) => void
}

export const useLiveStreams = create<LiveState>((set) => ({
  byScope: {},
  apply: (frame) =>
    set((state) => {
      const scope = threadScope(frame.channel_id, frame.parent_message_id)
      const streams = state.byScope[scope] ?? {}
      const existing = streams[frame.message_id]
      let next: LiveStream
      if (frame.catch_up) {
        next = {
          message_id: frame.message_id,
          run_id: frame.run_id,
          agent_id: frame.agent_id,
          seq: frame.seq,
          text: frame.text,
        }
      } else if (existing === undefined && frame.seq === 1) {
        next = {
          message_id: frame.message_id,
          run_id: frame.run_id,
          agent_id: frame.agent_id,
          seq: frame.seq,
          text: frame.text,
        }
      } else if (existing !== undefined && frame.seq === existing.seq + 1) {
        next = { ...existing, seq: frame.seq, text: existing.text + frame.text }
      } else {
        return state // stale or gapped; the server resends a catch-up
      }
      return {
        byScope: {
          ...state.byScope,
          [scope]: { ...streams, [frame.message_id]: next },
        },
      }
    }),
  clear: (scope, messageId) =>
    set((state) => {
      const streams = state.byScope[scope]
      if (streams?.[messageId] === undefined) return state
      const { [messageId]: _, ...rest } = streams
      return { byScope: { ...state.byScope, [scope]: rest } }
    }),
}))

/** One run's derived progress line, folded from
 *  `progress.state` frames. The daemon composes the text; the client
 *  only shows it. Keyed by scope, then by run. */
export interface RunProgress {
  run_id: string
  /** The message whose `progress` block holds the terminal text. */
  message_id: string
  agent_id: string
  /** The last folded frame; an older frame never overwrites it. */
  seq: number
  text: string
}

const NO_PROGRESS: Record<string, RunProgress> = {}

export function selectRunProgress(scope: string) {
  return (state: ProgressState): Record<string, RunProgress> =>
    state.byScope[scope] ?? NO_PROGRESS
}

export interface ProgressState {
  byScope: Record<string, Record<string, RunProgress>>
  /**
   * Fold one progress frame into its scope. Every frame carries the
   * whole line, so a newer `seq` replaces the held one and a stale
   * frame (a live frame that lost to a catch-up) is dropped.
   */
  apply: (frame: ProgressFrame) => void
  /** The progress row settled: the block now holds the same terminal
   *  text, so the live line is no longer needed. */
  settle: (scope: string, messageId: string) => void
}

export const useRunProgress = create<ProgressState>((set) => ({
  byScope: {},
  apply: (frame) =>
    set((state) => {
      const scope = threadScope(frame.channel_id, frame.parent_message_id)
      const runs = state.byScope[scope] ?? {}
      const existing = runs[frame.run_id]
      if (existing !== undefined && frame.seq <= existing.seq) return state
      const next: RunProgress = {
        run_id: frame.run_id,
        message_id: frame.message_id,
        agent_id: frame.agent_id,
        seq: frame.seq,
        text: frame.text,
      }
      return {
        byScope: {
          ...state.byScope,
          [scope]: { ...runs, [frame.run_id]: next },
        },
      }
    }),
  settle: (scope, messageId) =>
    set((state) => {
      const runs = state.byScope[scope]
      if (runs === undefined) return state
      const entries = Object.entries(runs).filter(
        ([, progress]) => progress.message_id !== messageId,
      )
      if (entries.length === Object.keys(runs).length) return state
      return {
        byScope: { ...state.byScope, [scope]: Object.fromEntries(entries) },
      }
    }),
}))

// Stable empty result so selectors do not loop useSyncExternalStore.
const NO_SENDS: PendingSend[] = []

export function selectPendingSends(scope: string) {
  return (state: PendingState): PendingSend[] =>
    state.byScope[scope] ?? NO_SENDS
}

export interface PendingState {
  byScope: Record<string, PendingSend[]>
  add: (scope: string, send: PendingSend) => void
  markFailed: (scope: string, pendingId: string) => void
  markPending: (scope: string, pendingId: string) => void
  remove: (scope: string, pendingId: string) => void
}

function update(
  byScope: Record<string, PendingSend[]>,
  scope: string,
  map: (sends: PendingSend[]) => PendingSend[],
): Record<string, PendingSend[]> {
  return { ...byScope, [scope]: map(byScope[scope] ?? []) }
}

export const usePendingSends = create<PendingState>((set) => ({
  byScope: {},
  add: (scope, send) =>
    set((state) => ({
      byScope: update(state.byScope, scope, (sends) => [...sends, send]),
    })),
  markFailed: (scope, pendingId) =>
    set((state) => ({
      byScope: update(state.byScope, scope, (sends) =>
        sends.map((s) =>
          s.pending_id === pendingId ? { ...s, state: 'failed' } : s,
        ),
      ),
    })),
  markPending: (scope, pendingId) =>
    set((state) => ({
      byScope: update(state.byScope, scope, (sends) =>
        sends.map((s) =>
          s.pending_id === pendingId ? { ...s, state: 'pending' } : s,
        ),
      ),
    })),
  remove: (scope, pendingId) =>
    set((state) => ({
      byScope: update(state.byScope, scope, (sends) =>
        sends.filter((s) => s.pending_id !== pendingId),
      ),
    })),
}))

/** Speaking (ADR-0020): one toggle per scope (a channel's top
 *  level, or one thread), off by default, turned on by holding the
 *  microphone and off through the toggle. `spoken` marks the messages
 *  this session read aloud; nothing of it is stored. */
export interface SpeakingState {
  byScope: Record<string, boolean>
  spoken: Record<string, true>
  enable: (scope: string) => void
  toggle: (scope: string) => void
  markSpoken: (messageId: string) => void
}

export const useSpeaking = create<SpeakingState>((set) => ({
  byScope: {},
  spoken: {},
  enable: (scope) =>
    set((state) =>
      state.byScope[scope] === true
        ? state
        : { byScope: { ...state.byScope, [scope]: true } },
    ),
  toggle: (scope) =>
    set((state) => ({
      byScope: { ...state.byScope, [scope]: state.byScope[scope] !== true },
    })),
  markSpoken: (messageId) =>
    set((state) => ({ spoken: { ...state.spoken, [messageId]: true } })),
}))

export function selectSpeaking(scope: string) {
  return (state: SpeakingState): boolean => state.byScope[scope] === true
}

/** One transcript line as the `call.transcript` event carries it.
 *  The Call record holds the lines that were said before the
 *  read; these are the ones said after it. */
export interface CallLine {
  at: number
  speaker: string
  text: string
}

const NO_LINES: CallLine[] = []

export function selectCallLines(callId: string) {
  return (state: CallTranscriptState): CallLine[] =>
    state.byCall[callId] ?? NO_LINES
}

/** The lines of every call this session watched, keyed by Call id. The
 *  record stays the source of truth; this is only what arrived since. */
export interface CallTranscriptState {
  byCall: Record<string, CallLine[]>
  /** Fold one `call.transcript` event. A line already held, by its time
   *  and its text, is not added twice: a refetch of the record and the
   *  event stream can carry the same line. */
  append: (callId: string, line: CallLine) => void
  /** The record now holds every line: drop the buffer. */
  clear: (callId: string) => void
}

export const useCallTranscripts = create<CallTranscriptState>((set) => ({
  byCall: {},
  append: (callId, line) =>
    set((state) => {
      const lines = state.byCall[callId] ?? NO_LINES
      const held = lines.some(
        (existing) => existing.at === line.at && existing.text === line.text,
      )
      if (held) return state
      return { byCall: { ...state.byCall, [callId]: [...lines, line] } }
    }),
  clear: (callId) =>
    set((state) => {
      if (state.byCall[callId] === undefined) return state
      const { [callId]: _gone, ...rest } = state.byCall
      return { byCall: rest }
    }),
}))

/** The call inspector (ADR-0022): a tenant of the
 *  inspector slot. It is transient — one
 *  open Call, or none — and it stays open across navigation, because
 *  listening is the reason it exists. The strip in the Thread is the
 *  only way back to it. */
export interface CallInspectorState {
  callId: string | null
  open: (callId: string) => void
  close: () => void
}

export const useCallInspector = create<CallInspectorState>((set) => ({
  callId: null,
  open: (callId) => set({ callId }),
  close: () => set({ callId: null }),
}))

/** The mail inspector (ADR-0019): a tenant of the
 *  inspector slot. It is transient — one open mail, or none — and it
 *  carries the block itself, because the block is the envelope and the
 *  words are read live.
 *
 *  Opening a mail gives the slot back from the Call: the call strip in
 *  the Thread is the way back to the Call, and the Call keeps running. */
export interface MailInspectorState {
  mail: MailBlockDto | null
  open: (mail: MailBlockDto) => void
  close: () => void
}

export const useMailInspector = create<MailInspectorState>((set) => ({
  mail: null,
  open: (mail) => {
    useCallInspector.setState({ callId: null })
    set({ mail })
  },
  close: () => set({ mail: null }),
}))


/** The handback countdown toasts: agent id -> seconds left, fed
 *  by the `screen.handback_countdown` events; any user input cancels
 *  it on the daemon, which clears it here. */
interface TakeoverCountdownState {
  byAgent: Record<string, number>
  start: (agentId: string, seconds: number) => void
  clear: (agentId: string) => void
}

export const useTakeoverCountdowns = create<TakeoverCountdownState>((set) => ({
  byAgent: {},
  start: (agentId, seconds) =>
    set((state) => ({ byAgent: { ...state.byAgent, [agentId]: seconds } })),
  clear: (agentId) =>
    set((state) => {
      const { [agentId]: _gone, ...rest } = state.byAgent
      return { byAgent: rest }
    }),
}))

/** The live Widget frames (ADR-0016). A widget is a whole HTML
 *  document in a frame, so the desk keeps at most `MAX_LIVE_WIDGETS` of
 *  them; every other Widget block shows its plain-text projection.
 *
 *  A block claims its slot when it mounts, and the oldest claim leaves
 *  when the list is full. Mount order is the reading order of the
 *  conversation, so the newest widgets keep the slots, and a block the
 *  timeline mounts again — because the reader scrolled to it — claims
 *  one again. */
export interface WidgetFrameState {
  live: string[]
  claim: (toolCallId: string) => void
  release: (toolCallId: string) => void
}

export function selectWidgetLive(toolCallId: string) {
  return (state: WidgetFrameState): boolean => state.live.includes(toolCallId)
}

export const useWidgetFrames = create<WidgetFrameState>((set) => ({
  live: [],
  claim: (toolCallId) =>
    set((state) => {
      if (state.live.includes(toolCallId)) return state
      const live = [...state.live, toolCallId]
      return { live: live.slice(Math.max(0, live.length - MAX_LIVE_WIDGETS)) }
    }),
  release: (toolCallId) =>
    set((state) => {
      if (!state.live.includes(toolCallId)) return state
      return { live: state.live.filter((held) => held !== toolCallId) }
    }),
}))

/** The Widget views the daemon tore down (ADR-0016). A Run that
 *  ends drops its views, so the host tells each frame with the
 *  extension's `ui/resource-teardown` and stops proxying for it. */
export interface WidgetTeardownState {
  torn: Record<string, true>
  tearDown: (toolCallIds: readonly string[]) => void
}

export function selectWidgetTorn(toolCallId: string) {
  return (state: WidgetTeardownState): boolean => state.torn[toolCallId] === true
}

export const useWidgetTeardowns = create<WidgetTeardownState>((set) => ({
  torn: {},
  tearDown: (toolCallIds) =>
    set((state) => {
      const torn = { ...state.torn }
      for (const id of toolCallIds) torn[id] = true
      return { torn }
    }),
}))

/** The screenshot the Desk panel shows. A step of the work
 *  record carries the Artifact of a screen it took; the tile asks the
 *  panel to scroll to that screenshot. The panel owns the scrolling,
 *  so the store holds only the wish. */
export interface DeskFocusState {
  screenshotId: string | null
  show: (screenshotId: string) => void
  clear: () => void
}

export const useDeskFocus = create<DeskFocusState>((set) => ({
  screenshotId: null,
  show: (screenshotId) => set({ screenshotId }),
  clear: () => set({ screenshotId: null }),
}))

/** The search of one conversation. The Thread header holds the
 *  words, the timeline keeps the rows that carry them. The query is
 *  chrome, not a view, so it stays out of the URL. */
export interface ThreadSearchState {
  byChannel: Record<string, string>
  set: (channelId: string, query: string) => void
}

export function selectThreadQuery(channelId: string) {
  return (state: ThreadSearchState): string => state.byChannel[channelId] ?? ''
}

export const useThreadSearch = create<ThreadSearchState>((set) => ({
  byChannel: {},
  set: (channelId, query) =>
    set((state) => ({ byChannel: { ...state.byChannel, [channelId]: query } })),
}))
