// The pending-send store: lifecycle transitions, and a stable snapshot
// for a scope without sends (an unstable one loops React's
// useSyncExternalStore and crashes the page). The live-stream
// store: catch-up replacement, seq-deduped delta folding, and thread
// scoping. The run-progress store: whole-line replacement, stale
// frames dropped, and the settled row's buffer released.

import { describe, expect, it } from 'vitest'

import type { DeltaFrame, ProgressFrame } from '../api/client'
import {
  selectLiveStreams,
  selectPendingSends,
  selectRunProgress,
  selectThreadQuery,
  useDeskFocus,
  useLiveStreams,
  usePendingSends,
  useRunProgress,
  useThreadSearch,
} from './stores'

describe('usePendingSends', () => {
  it('returns the same empty reference for a channel with no sends', () => {
    const first = selectPendingSends('ch-none')(usePendingSends.getState())
    const second = selectPendingSends('ch-none')(usePendingSends.getState())
    expect(first).toBe(second)
  })

  it('adds, fails, retries, and removes a send', () => {
    const store = usePendingSends.getState()
    const send = {
      pending_id: 'p-1',
      text: 'hi',
      created_at: 1,
      state: 'pending' as const,
    }
    store.add('ch-1', send)
    expect(selectPendingSends('ch-1')(usePendingSends.getState())).toEqual([send])

    store.markFailed('ch-1', 'p-1')
    expect(
      selectPendingSends('ch-1')(usePendingSends.getState())[0].state,
    ).toBe('failed')

    store.markPending('ch-1', 'p-1')
    expect(
      selectPendingSends('ch-1')(usePendingSends.getState())[0].state,
    ).toBe('pending')

    store.remove('ch-1', 'p-1')
    expect(selectPendingSends('ch-1')(usePendingSends.getState())).toEqual([])
  })
})

function delta(overrides: Partial<DeltaFrame>): DeltaFrame {
  return {
    channel_id: 'ch-1',
    message_id: 'msg-1',
    run_id: 'run-1',
    agent_id: 'ag-1',
    parent_message_id: null,
    seq: 1,
    text: 'Hel',
    catch_up: false,
    ...overrides,
  }
}

describe('useLiveStreams', () => {
  it('returns the same empty reference for a channel with no streams', () => {
    const first = selectLiveStreams('ch-none')(useLiveStreams.getState())
    const second = selectLiveStreams('ch-none')(useLiveStreams.getState())
    expect(first).toBe(second)
  })

  it('folds sequential deltas and ignores replays', () => {
    const store = useLiveStreams.getState()
    store.apply(delta({ channel_id: 'ch-fold', seq: 1, text: 'Hel' }))
    store.apply(delta({ channel_id: 'ch-fold', seq: 2, text: 'lo' }))
    // A replayed frame (after a catch-up raced a buffered delta).
    store.apply(delta({ channel_id: 'ch-fold', seq: 2, text: 'lo' }))
    const streams = selectLiveStreams('ch-fold')(useLiveStreams.getState())
    expect(streams['msg-1'].text).toBe('Hello')
    expect(streams['msg-1'].seq).toBe(2)
  })

  it('replaces the buffer with a catch-up frame', () => {
    const store = useLiveStreams.getState()
    store.apply(
      delta({ channel_id: 'ch-cu', seq: 3, text: 'Hello wor', catch_up: true }),
    )
    store.apply(delta({ channel_id: 'ch-cu', seq: 4, text: 'ld' }))
    const streams = selectLiveStreams('ch-cu')(useLiveStreams.getState())
    expect(streams['msg-1'].text).toBe('Hello world')
  })

  it('drops a gapped delta until the next catch-up', () => {
    const store = useLiveStreams.getState()
    store.apply(delta({ channel_id: 'ch-gap', seq: 5, text: 'late' }))
    expect(
      selectLiveStreams('ch-gap')(useLiveStreams.getState())['msg-1'],
    ).toBeUndefined()
  })

  it('keys a thread-reply delta by its thread scope', () => {
    const store = useLiveStreams.getState()
    store.apply(
      delta({ channel_id: 'ch-thread', parent_message_id: 'msg-root' }),
    )
    expect(
      selectLiveStreams('ch-thread')(useLiveStreams.getState())['msg-1'],
    ).toBeUndefined()
    expect(
      selectLiveStreams('ch-thread/msg-root')(useLiveStreams.getState())[
        'msg-1'
      ].text,
    ).toBe('Hel')
  })

  it('clears a settled stream', () => {
    const store = useLiveStreams.getState()
    store.apply(delta({ channel_id: 'ch-clear' }))
    store.clear('ch-clear', 'msg-1')
    expect(
      selectLiveStreams('ch-clear')(useLiveStreams.getState())['msg-1'],
    ).toBeUndefined()
  })
})

function progress(overrides: Partial<ProgressFrame>): ProgressFrame {
  return {
    channel_id: 'ch-1',
    message_id: 'msg-p1',
    run_id: 'run-1',
    agent_id: 'ag-1',
    parent_message_id: null,
    seq: 1,
    text: 'Thinking…',
    ...overrides,
  }
}

describe('useRunProgress', () => {
  it('returns the same empty reference for a channel with no runs', () => {
    const first = selectRunProgress('ch-none')(useRunProgress.getState())
    const second = selectRunProgress('ch-none')(useRunProgress.getState())
    expect(first).toBe(second)
  })

  it('replaces the line with each newer frame', () => {
    const store = useRunProgress.getState()
    store.apply(progress({ channel_id: 'ch-line', seq: 1 }))
    store.apply(
      progress({ channel_id: 'ch-line', seq: 2, text: 'Running `git status`…' }),
    )
    const runs = selectRunProgress('ch-line')(useRunProgress.getState())
    expect(runs['run-1'].text).toBe('Running `git status`…')
    expect(runs['run-1'].seq).toBe(2)
  })

  it('drops a frame that lost to a newer one', () => {
    const store = useRunProgress.getState()
    store.apply(progress({ channel_id: 'ch-stale', seq: 4, text: 'Done' }))
    store.apply(progress({ channel_id: 'ch-stale', seq: 3, text: 'Thinking…' }))
    expect(
      selectRunProgress('ch-stale')(useRunProgress.getState())['run-1'].text,
    ).toBe('Done')
  })

  it('keys a thread-bound run by its thread scope', () => {
    const store = useRunProgress.getState()
    store.apply(
      progress({ channel_id: 'ch-thread', parent_message_id: 'msg-root' }),
    )
    expect(
      selectRunProgress('ch-thread')(useRunProgress.getState())['run-1'],
    ).toBeUndefined()
    expect(
      selectRunProgress('ch-thread/msg-root')(useRunProgress.getState())[
        'run-1'
      ].text,
    ).toBe('Thinking…')
  })

  it('releases the buffer of the row that settled', () => {
    const store = useRunProgress.getState()
    store.apply(progress({ channel_id: 'ch-settle' }))
    store.settle('ch-settle', 'other-message')
    expect(
      selectRunProgress('ch-settle')(useRunProgress.getState())['run-1'],
    ).toBeDefined()
    store.settle('ch-settle', 'msg-p1')
    expect(
      selectRunProgress('ch-settle')(useRunProgress.getState())['run-1'],
    ).toBeUndefined()
  })
})

// The screenshot the Desk panel shows. A step of the work
// record names one screenshot, and the panel scrolls to it.
describe('useDeskFocus', () => {
  it('holds the screenshot the reader asked for, and gives it back', () => {
    useDeskFocus.getState().show('art-1')
    expect(useDeskFocus.getState().screenshotId).toBe('art-1')

    useDeskFocus.getState().clear()
    expect(useDeskFocus.getState().screenshotId).toBeNull()
  })
})

// The search of one conversation: the header asks, the timeline
// answers. The query is per channel, so another channel is unfiltered.
describe('useThreadSearch', () => {
  it('keeps one query per channel', () => {
    useThreadSearch.getState().set('ch-1', 'paris')
    expect(selectThreadQuery('ch-1')(useThreadSearch.getState())).toBe('paris')
    expect(selectThreadQuery('ch-2')(useThreadSearch.getState())).toBe('')

    useThreadSearch.getState().set('ch-1', '')
    expect(selectThreadQuery('ch-1')(useThreadSearch.getState())).toBe('')
  })
})
