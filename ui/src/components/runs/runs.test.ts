import { describe, expect, it } from 'vitest'

import type { RunDto, RunEventDto } from '../../api/client'
import {
  chipCount,
  dayLabel,
  failureText,
  groupByDay,
  modelRequests,
  runDuration,
  runStateBadge,
  runSteps,
  stepDuration,
  summariseArguments,
  summariseResult,
  triggerSentence,
  triggerText,
} from './runs'

const DAY = 86_400_000
const NOW = new Date('2026-05-04T12:00:00Z').getTime()

function run(fields: Partial<RunDto> = {}): RunDto {
  return {
    id: 'run-1',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    root_message_id: null,
    trigger_kind: 'message',
    trigger_ref: 'message-1',
    hop_count: 0,
    state: 'completed',
    error: null,
    started_at: NOW,
    ended_at: NOW + 600,
    created_at: NOW,
    duration_ms: 600,
    ...fields,
  }
}

function event(fields: Partial<RunEventDto>): RunEventDto {
  return {
    id: 'e1',
    event_type: 'tool.called',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    created_at: NOW,
    payload: {},
    ...fields,
  }
}

describe('the run reads in words', () => {
  it('names the trigger with the channel it came from', () => {
    expect(triggerText(run(), 'Sage')).toBe('A message in Sage')
    expect(triggerText(run({ trigger_kind: 'schedule' }), null)).toBe('A schedule')
    expect(triggerText(run({ trigger_kind: 'web_hook' }), null)).toBe('web hook')
  })

  it('lowers only the first letter inside a sentence', () => {
    expect(triggerSentence(run(), 'Sage')).toBe('a message in Sage')
  })

  it('gives the state a word and a semantic hue', () => {
    expect(runStateBadge('completed')).toEqual({ label: 'Done', tone: 'working' })
    expect(runStateBadge('running')).toEqual({ label: 'Working', tone: 'on-call' })
    expect(runStateBadge('reflecting')).toEqual({ label: 'Updating memory', tone: 'neutral' })
    expect(runStateBadge('failed')).toEqual({ label: 'Failed', tone: 'failed' })
    expect(runStateBadge('waiting_for_approval').tone).toBe('waiting')
  })

  it('says how long the run took, and says so while it runs', () => {
    expect(runDuration(run({ duration_ms: 600 }))).toBe('1 s')
    expect(runDuration(run({ duration_ms: 64_000 }))).toBe('1 min 4 s')
    expect(runDuration(run({ duration_ms: null }))).toBe('In progress')
  })
})

describe('the failure reads in plain words', () => {
  it.each([
    ['agent_missing', 'Ended because the sprite could not be loaded'],
    ['model_missing', 'Ended because the model could not be loaded'],
    ['context_failed', 'Ended because the conversation could not be prepared'],
    ['tool_failed', 'Ended because a tool failed'],
    ['access_changed', 'Ended because access changed'],
    ['publication_rejected', 'Ended because the reply was rejected'],
    ['lease_failed', 'Ended because the computer could not be reserved'],
    ['daemon_restarted', 'Ended because the daemon restarted'],
    ['model_failed', 'Ended because the model request failed'],
    ['turn_limit', 'Ended because the turn limit was reached'],
    ['unknown', 'Ended with an error'],
  ] as const)('reads %s without guessing from the error detail', (failure_kind, text) => {
    expect(failureText(run({ state: 'failed', failure_kind, error: 'unrelated detail' }))).toBe(text)
  })
  it('reads a failed call', () => {
    expect(failureText(run({ state: 'failed', failure_kind: 'call_failed' }))).toBe('Ended because the phone call failed')
  })
  it('uses the fallback when no kind was recorded', () => {
    expect(failureText(run({ state: 'failed', error: 'daemon restarted' }))).toBe('Ended with an error')
  })
  it('has no failure line for a run that worked', () => {
    expect(failureText(run())).toBeNull()
  })
})

describe('the record groups by day', () => {
  it('names today, yesterday and the date', () => {
    expect(dayLabel(NOW, NOW)).toBe('Today')
    expect(dayLabel(NOW - DAY, NOW)).toBe('Yesterday')
    expect(dayLabel(NOW - 4 * DAY, NOW)).toContain('April')
  })

  it('puts the newest day and the newest run first', () => {
    const days = groupByDay(
      [
        run({ id: 'old', created_at: NOW - DAY }),
        run({ id: 'new', created_at: NOW }),
        run({ id: 'newer', created_at: NOW + 10 }),
      ],
      NOW,
    )
    expect(days.map((day) => day.label)).toEqual(['Today', 'Yesterday'])
    expect(days[0].runs.map((each) => each.id)).toEqual(['newer', 'new'])
  })
})

describe('the filter chips count', () => {
  const runs = [
    run({ id: 'a', agent_id: 'agent-1', state: 'completed' }),
    run({ id: 'b', agent_id: 'agent-1', state: 'failed' }),
    run({ id: 'c', agent_id: 'agent-2', state: 'failed' }),
  ]

  it('counts every run when nothing is filtered', () => {
    const filters = { agentId: null, channelId: null, state: null }
    expect(chipCount(runs, filters, 'state', 'failed')).toBe(2)
    expect(chipCount(runs, filters, 'state', null)).toBe(3)
  })

  it('holds the other filters, so a count moves when they change', () => {
    const byAgent = { agentId: 'agent-1', channelId: null, state: null }
    expect(chipCount(runs, byAgent, 'state', 'failed')).toBe(1)
    expect(chipCount(runs, byAgent, 'state', 'completed')).toBe(1)
  })

  it('drops the filter of its own chip, so a chip counts what clicking it gives', () => {
    const byState = { agentId: null, channelId: null, state: 'completed' }
    expect(chipCount(runs, byState, 'state', 'failed')).toBe(2)
  })
})

describe('a step summarises to a line', () => {
  const called = event({
    id: 'c1',
    payload: {
      name: 'computer',
      arguments: JSON.stringify({
        actions: [{ type: 'click', x: 10 }, { type: 'wait' }],
        call_id: 'call_1',
        id: 'cu_1',
        status: 'completed',
        type: 'computer_call',
      }),
    },
  })

  it('reads the arguments the daemon wrote as JSON text', () => {
    expect(summariseArguments(called)).toBe('actions: click, wait')
  })

  it('drops nothing but the plumbing, and holds one line', () => {
    const long = event({
      payload: { name: 'shell', arguments: { command: 'x'.repeat(200) } },
    })
    expect(summariseArguments(long).length).toBeLessThanOrEqual(90)
    expect(summariseArguments(long).endsWith('…')).toBe(true)
  })

  it('says so when the call carried no argument', () => {
    expect(summariseArguments(event({ payload: { name: 'now' } }))).toBe('No argument')
  })

  it('summarises the result, the screenshots and the failure', () => {
    expect(summariseResult(null)).toBe('No result')
    expect(
      summariseResult(event({ event_type: 'tool.completed', payload: { ok: true } })),
    ).toBe('Done')
    expect(
      summariseResult(
        event({
          event_type: 'tool.completed',
          payload: { ok: true, artifact_ids: ['a', 'b'] },
        }),
      ),
    ).toBe('Done, with 2 screenshots')
    expect(
      summariseResult(
        event({
          event_type: 'tool.completed',
          payload: { ok: false, error: 'the window was gone' },
        }),
      ),
    ).toBe('Failed: the window was gone')
  })

  it('joins each call to the completion that settles it, in order', () => {
    const steps = runSteps([
      event({ id: 'a', payload: { name: 'computer', arguments: '{"actions":[]}' } }),
      event({ id: 'b', payload: { name: 'computer', arguments: '{"actions":[]}' } }),
      event({
        id: 'c',
        event_type: 'tool.completed',
        payload: { name: 'computer', ok: true, duration_ms: 97 },
      }),
      event({
        id: 'd',
        event_type: 'tool.completed',
        payload: { name: 'computer', ok: false, error: 'gone', duration_ms: 2_000 },
      }),
      event({ id: 'e', event_type: 'turn.completed', payload: { turn: 0 } }),
    ])
    expect(steps).toHaveLength(2)
    expect(steps[0].completed?.id).toBe('c')
    expect(steps[0].failed).toBe(false)
    expect(stepDuration(steps[0])).toBe('97 ms')
    expect(steps[1].failed).toBe(true)
    expect(steps[1].result).toBe('Failed: gone')
    expect(stepDuration(steps[1])).toBe('2 s')
  })

  it('holds a call the run never settled', () => {
    const steps = runSteps([event({ id: 'a', payload: { name: 'computer' } })])
    expect(steps[0].result).toBe('No result')
    expect(stepDuration(steps[0])).toBe('—')
  })
})

describe('a model request summarises to a line', () => {
  const requested = (phase: string, phaseRequest: number, fields: Record<string, unknown> = {}) =>
    event({
      id: `${phase}-${phaseRequest}`,
      event_type: 'model.requested',
      payload: {
        phase,
        phase_request: phaseRequest,
        model_alias: 'default',
        model_candidates: ['openrouter/qwen/qwen3.8-27b'],
        estimated_input_tokens: 41_200,
        input_allowance: 112_000,
        max_output_tokens: null,
        messages: 14,
        tools: 23,
        images: 0,
        ...fields,
      },
    })
  const completed = (phase: string, phaseRequest: number, fields: Record<string, unknown>) =>
    event({
      id: `${phase}-${phaseRequest}-done`,
      event_type: 'model.completed',
      payload: { phase, phase_request: phaseRequest, ...fields },
    })

  it('reads the route, the size, the output limit and the counts', () => {
    const [request] = modelRequests([requested('reply', 0)])
    expect(request.label).toBe('Turn 1')
    expect(request.summary).toBe(
      'openrouter/qwen/qwen3.8-27b · about 41,200 of 112,000 input tokens · max output not set · 14 messages · 23 tools',
    )
    expect(request.error).toBeNull()
    expect(request.failed).toBe(false)
  })

  it('marks a request that the budget check rejected as failed', () => {
    const [request] = modelRequests([
      requested('reply', 0),
      completed('reply', 0, { outcome: 'rejected', error: 'model request needs at most 9 tokens' }),
    ])
    expect(request.failed).toBe(true)
  })

  it('names the serving model, the output limit and the images when there are some', () => {
    const [request] = modelRequests([
      requested('compaction', 0, { max_output_tokens: 16_384, images: 2, model_candidates: ['a/x', 'b/y'] }),
      completed('compaction', 0, { outcome: 'completed', provider: 'b', model: 'y' }),
    ])
    expect(request.label).toBe('Compaction 1')
    expect(request.summary).toBe(
      'b/y · about 41,200 of 112,000 input tokens · max output 16,384 · 14 messages · 23 tools · 2 images',
    )
  })

  it('joins each request to the completion of the same phase and number', () => {
    const requests = modelRequests([
      requested('reply', 0),
      completed('reply', 0, { outcome: 'completed' }),
      requested('reply', 1),
      requested('reflection', 0),
      completed('reflection', 0, { outcome: 'completed' }),
      completed('reply', 1, { outcome: 'failed', error: 'status 402: needs more credits' }),
    ])
    expect(requests.map((request) => request.label)).toEqual(['Turn 1', 'Turn 2', 'Reflection 1'])
    expect(requests.map((request) => request.failed)).toEqual([false, true, false])
    expect(requests[0].error).toBeNull()
    expect(requests[1].error).toBe('status 402: needs more credits')
    expect(requests[2].error).toBeNull()
  })

  it('names the alias when several candidates could serve and none answered', () => {
    const [request] = modelRequests([
      requested('reply', 0, { model_candidates: ['a/x', 'b/y'], estimated_input_tokens: null, input_allowance: null }),
    ])
    expect(request.summary).toBe('default · max output not set · 14 messages · 23 tools')
  })
})
