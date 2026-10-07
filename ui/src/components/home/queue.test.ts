// What Home adds to an item of the daemon's Needs-You Queue: the
// detail that depends on the reader's locale, the action of each kind,
// and the message that the Call back action writes.

import { describe, expect, it } from 'vitest'

import type { NeedsYouItem } from '../../api/client'
import { formatClock } from '../../timeline'
import { callBackDraft, isDismissible, queueAction, queueDetail, type QueueItem } from './queue'

const NOW = Date.UTC(2026, 8, 5, 12, 0, 0)

type Item<Kind extends NeedsYouItem['kind']> = Extract<NeedsYouItem, { kind: Kind }>

const approval: Item<'approval'> = {
  kind: 'approval',
  id: 'request:request-1',
  agent_id: 'agent-1',
  line: 'Sage needs your approval',
  url: '/c/channel-1',
  at: NOW,
  request_id: 'request-1',
  request_kind: 'tool_action',
  title: 'Open a file',
  body: 'host__read',
}

const waiting: Item<'waiting'> = {
  kind: 'waiting',
  id: 'run:run-9',
  agent_id: 'agent-1',
  line: 'Sage waits for your answer',
  url: '/c/channel-1',
  at: NOW,
  run_id: 'run-9',
  channel_id: 'channel-1',
}

function call(fields: Partial<Item<'call'>> = {}): Item<'call'> {
  return {
    kind: 'call',
    id: 'call:call-1',
    agent_id: 'agent-1',
    line: 'Sage missed a call from +14155550199',
    url: '/',
    at: NOW,
    call_id: 'call-1',
    remote_e164: '+14155550199',
    left_message: false,
    ...fields,
  }
}

function failed(fields: Partial<Item<'failed'>> = {}): Item<'failed'> {
  return {
    kind: 'failed',
    id: 'run:run-2',
    agent_id: 'agent-1',
    line: 'Sage could not finish the work',
    url: '/runs/run-2',
    at: NOW,
    run_id: 'run-2',
    channel_id: 'channel-1',
    failure_kind: null,
    ...fields,
  }
}

function keypad(suspendedUntil: number): Item<'keypad'> {
  return {
    kind: 'keypad',
    id: 'keypad',
    line: 'Callers entered a wrong keypad code 6 times',
    url: '/',
    at: suspendedUntil,
    failed_attempts: 6,
    suspended_until: suspendedUntil,
  }
}

describe('queueDetail', () => {
  it('says when a keypad delay ends', () => {
    const until = NOW + 60_000

    expect(queueDetail(keypad(until), NOW)).toBe(
      `Pagis checks no keypad code until ${formatClock(until)}. Calls are still answered, as Unknown.`,
    )
  })

  it('names the day of a keypad delay that ends on another day', () => {
    // Thirty hours ends on another day in every time zone.
    const until = NOW + 30 * 3_600_000

    const day = new Date(until).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
    expect(queueDetail(keypad(until), NOW)).toBe(
      `Pagis checks no keypad code until ${day}, ${formatClock(until)}. Calls are still answered, as Unknown.`,
    )
  })

  it('says when a keypad delay ended, while the count stays', () => {
    const until = NOW - 60_000

    expect(queueDetail(keypad(until), NOW)).toBe(
      `The delay ended at ${formatClock(until)}. The next wrong code starts a longer delay.`,
    )
  })

  it('says why a run failed, from the kind of the failure', () => {
    expect(queueDetail(failed({ failure_kind: 'call_failed' }), NOW)).toBe(
      'Ended because the phone call failed',
    )
    expect(queueDetail(failed({ failure_kind: null }), NOW)).toBe('Ended with an error')
  })

  it('says whether the caller of a missed call left a message', () => {
    expect(queueDetail(call(), NOW)).toBe('Nobody answered.')
    expect(queueDetail(call({ left_message: true }), NOW)).toBe('They left a message.')
  })

  it('gives a run that waits no caption', () => {
    expect(queueDetail(waiting, NOW)).toBeNull()
  })
})

describe('queueAction', () => {
  it('names the control that settles each kind', () => {
    const items: QueueItem[] = [waiting, keypad(NOW), call(), failed()]

    expect(items.map(queueAction)).toEqual([
      'Open conversation',
      'Clear the count',
      'Call back',
      'Open run',
    ])
  })
})

describe('isDismissible', () => {
  it('lets the reader dismiss only a missed call and a failure', () => {
    const items: QueueItem[] = [approval, waiting, keypad(NOW), call(), failed()]

    expect(items.filter(isDismissible).map((item) => item.kind)).toEqual(['call', 'failed'])
  })
})

describe('callBackDraft', () => {
  it('asks the agent to call the number back, in the words of the reader', () => {
    expect(callBackDraft(call())).toBe(
      'Please call +14155550199 back. They called and nobody answered.',
    )
  })

  it('says the caller left a message, so the draft agrees with the row', () => {
    expect(callBackDraft(call({ left_message: true }))).toBe(
      'Please call +14155550199 back. They called and left a message.',
    )
  })
})
