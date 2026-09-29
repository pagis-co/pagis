// The rules Home reads: what joins the queue, in which order,
// what each line says, and which agents get a thumbnail.

import { describe, expect, it } from 'vitest'

import type { CallSummaryDto, KeypadCodeDto, RequestDto, RunDto } from '../../api/client'
import type { LiveRun } from '../../state/presence'
import { formatClock } from '../../timeline'
import {
  buildQueue,
  callBackDraft,
  missedCallItem,
  queueAction,
  queueLine,
  type KeypadItem,
  type QueueItem,
} from './queue'

const NOW = Date.UTC(2026, 8, 5, 12, 0, 0)
const YESTERDAY = NOW - 86_400_000

function run(fields: Partial<RunDto>): RunDto {
  return {
    id: 'run-1',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    root_message_id: null,
    trigger_kind: 'message',
    trigger_ref: null,
    hop_count: 0,
    state: 'completed',
    error: null,
    started_at: NOW,
    ended_at: NOW,
    created_at: NOW,
    duration_ms: 600,
    ...fields,
  }
}

function call(fields: Partial<CallSummaryDto>): CallSummaryDto {
  return {
    id: 'call-1',
    agent_id: 'agent-1',
    agent_name: 'Sage',
    own_e164: '+14155550123',
    direction: 'inbound',
    remote_e164: '+14155550199',
    purpose: '',
    tier: 'unknown',
    state: 'ended',
    outcome: 'no_answer',
    ended_reason: 'no_answer',
    classification: null,
    message_left: false,
    recording_artifact_id: null,
    created_at: NOW,
    answered_at: null,
    ended_at: NOW,
    ...fields,
  }
}

function request(fields: Partial<RequestDto>): RequestDto {
  return {
    id: 'request-1',
    agent_id: 'agent-1',
    run_id: 'run-1',
    kind: 'tool_action',
    state: 'pending',
    payload: { action_title: 'Open a file', body: 'host__read' },
    created_at: NOW,
    decided_at: null,
    ...fields,
  }
}

function live(fields: Partial<LiveRun>): LiveRun {
  return {
    runId: 'run-9',
    agentId: 'agent-1',
    channelId: 'channel-1',
    originChannelId: null,
    state: 'waiting_for_user',
    caption: 'Working on your message',
    ...fields,
  }
}


describe('buildQueue', () => {
  it('takes the pending approvals, the waiting runs and the failures of today', () => {
    const queue = buildQueue({
      requests: [request({}), request({ id: 'request-2', state: 'approved' })],
      liveRuns: [live({}), live({ runId: 'run-8', state: 'running' })],
      failedRuns: [
        run({ id: 'run-2', state: 'failed', error: 'the request timed out' }),
        run({ id: 'run-3', state: 'completed' }),
      ],
      calls: [],
      now: NOW,
    })

    expect(queue.map((item) => `${item.kind}:${item.id}`)).toEqual([
      'approval:request-1',
      'waiting:run-9',
      'failed:run-2',
    ])
  })

  it('leaves out a failure from an earlier day', () => {
    const queue = buildQueue({
      requests: [],
      liveRuns: [],
      failedRuns: [
        run({ id: 'old', state: 'failed', ended_at: YESTERDAY, created_at: YESTERDAY }),
      ],
      calls: [],
      now: NOW,
    })

    expect(queue).toEqual([])
  })

  it('takes the inbound calls of today that nobody answered, before the failures', () => {
    const queue = buildQueue({
      requests: [],
      liveRuns: [],
      failedRuns: [run({ id: 'plain', state: 'failed', error: 'the request timed out' })],
      calls: [
        call({ id: 'missed', outcome: 'no_answer' }),
        call({ id: 'voicemail', outcome: 'voicemail', ended_at: NOW - 1000 }),
        call({ id: 'answered', outcome: 'answered' }),
        call({ id: 'outbound', direction: 'outbound', outcome: 'busy' }),
        call({ id: 'live', state: 'live', outcome: null, ended_at: null }),
        call({ id: 'old', outcome: 'failed', ended_at: YESTERDAY }),
      ],
      now: NOW,
    })

    expect(queue.map((item) => `${item.kind}:${item.id}`)).toEqual([
      'call:missed',
      'call:voicemail',
      'failed:plain',
    ])
  })

  it('does not read a failed run with a call trigger as a missed call', () => {
    const queue = buildQueue({
      requests: [],
      liveRuns: [],
      failedRuns: [
        run({ id: 'call-run', state: 'failed', trigger_kind: 'call', error: 'no_answer' }),
      ],
      calls: [],
      now: NOW,
    })

    expect(queue.map((item) => item.kind)).toEqual(['failed'])
  })
})

function keypad(fields: Partial<KeypadCodeDto>): KeypadCodeDto {
  return { configured: true, failed_attempts: 0, suspended_until: null, ...fields }
}

/** The keypad notice of a queue, which the test expects there. */
function keypadNotice(queue: QueueItem[]): KeypadItem {
  const notice = queue.find((item): item is KeypadItem => item.kind === 'keypad')
  if (notice === undefined) throw new Error('the queue holds no keypad notice')
  return notice
}

describe('the keypad notice', () => {
  it('joins the queue when a delay starts, and says when the delay ends', () => {
    const until = NOW + 60_000
    const queue = buildQueue({
      requests: [request({})],
      liveRuns: [],
      failedRuns: [],
      calls: [call({})],
      keypad: keypad({ failed_attempts: 6, suspended_until: until }),
      now: NOW,
    })

    expect(queue.map((item) => item.kind)).toEqual(['approval', 'keypad', 'call'])
    const notice = keypadNotice(queue)
    expect(queueLine(notice, 'Sage')).toBe('Callers entered a wrong keypad code 6 times')
    expect(notice.detail).toBe(
      `Pagis checks no keypad code until ${formatClock(until)}. Calls are still answered, as Unknown.`,
    )
    expect(queueAction(notice)).toBe('Clear the count')
  })

  it('names the day of a delay that ends on another day', () => {
    // Thirty hours ends on another day in every time zone.
    const until = NOW + 30 * 3_600_000
    const notice = keypadNotice(
      buildQueue({
        requests: [],
        liveRuns: [],
        failedRuns: [],
        calls: [],
        keypad: keypad({ failed_attempts: 17, suspended_until: until }),
        now: NOW,
      }),
    )

    const day = new Date(until).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
    expect(notice.detail).toBe(
      `Pagis checks no keypad code until ${day}, ${formatClock(until)}. Calls are still answered, as Unknown.`,
    )
  })

  it('stays after the delay ends, until the count is cleared', () => {
    const until = NOW - 60_000
    const notice = keypadNotice(
      buildQueue({
        requests: [],
        liveRuns: [],
        failedRuns: [],
        calls: [],
        keypad: keypad({ failed_attempts: 7, suspended_until: until }),
        now: NOW,
      }),
    )

    expect(notice.detail).toBe(
      `The delay ended at ${formatClock(until)}. The next wrong code starts a longer delay.`,
    )
  })

  it('stays out of the queue while no delay has started', () => {
    const queue = buildQueue({
      requests: [],
      liveRuns: [],
      failedRuns: [],
      calls: [],
      keypad: keypad({ failed_attempts: 5 }),
      now: NOW,
    })

    expect(queue).toEqual([])
  })
})

describe('queueLine', () => {
  it('says what each kind asks of the reader', () => {
    const [approval, waiting, missed, failed] = buildQueue({
      requests: [request({})],
      liveRuns: [live({})],
      failedRuns: [run({ id: 'run-2', state: 'failed' })],
      calls: [call({})],
      now: NOW,
    })

    expect(queueLine(approval, 'Sage')).toBe('Sage needs your approval')
    expect(queueLine(waiting, 'Sage')).toBe('Sage waits for your answer')
    expect(queueLine(missed, 'Sage')).toBe('Sage missed a call from +14155550199')
    expect(queueLine(failed, 'Sage')).toBe('Sage could not finish the work')
    expect(queueAction(waiting)).toBe('Open conversation')
    expect(queueAction(missed)).toBe('Call back')
    expect(queueAction(failed)).toBe('Open run')
  })
})

describe('callBackDraft', () => {
  it('asks the agent to call the number back, in the words of the reader', () => {
    expect(callBackDraft(missedCallItem(call({})))).toBe(
      'Please call +14155550199 back. They called and nobody answered.',
    )
  })

  it('says the caller left a message, so the draft agrees with the row', () => {
    const item = missedCallItem(call({ outcome: 'voicemail' }))

    expect(item.detail).toBe('They left a message.')
    expect(callBackDraft(item)).toBe(
      'Please call +14155550199 back. They called and left a message.',
    )
  })
})
