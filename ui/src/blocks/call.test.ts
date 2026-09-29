// The words the call surfaces read from the Call record.

import { describe, expect, it } from 'vitest'

import type { CallDto } from '../api/client'
import {
  callDuration,
  callFailed,
  callStateChip,
  failureText,
  classificationText,
  endedReason,
  formatDuration,
  formatE164,
  lastSaid,
  mergeTranscript,
  outcomeText,
  speakerLabel,
  whoLine,
} from './call'

function call(overrides: Partial<CallDto> = {}): CallDto {
  return {
    id: 'call_1',
    agent_id: 'agent_1',
    agent_name: 'Robin',
    own_e164: '+14155550123',
    direction: 'outbound',
    remote_e164: '+14155559981',
    purpose: 'Move the cleaning to a morning.',
    tools: ['memory_read'],
    tier: 'unknown',
    state: 'live',
    outcome: null,
    ended_reason: null,
    classification: null,
    message_left: false,
    transcript: [],
    recording_artifact_id: null,
    created_at: 1_000,
    answered_at: 2_000,
    ended_at: null,
    ...overrides,
  } as CallDto
}

describe('the numbers and the clock', () => {
  it('reads a North American number in groups', () => {
    expect(formatE164('+14155559981')).toBe('+1 415 555 9981')
  })

  it('leaves any other number as it was dialed', () => {
    expect(formatE164('+442079460123')).toBe('+442079460123')
  })

  it('reads a span as minutes and seconds', () => {
    expect(formatDuration(0)).toBe('0:00')
    expect(formatDuration(134_000)).toBe('2:14')
    expect(formatDuration(3_723_000)).toBe('1:02:03')
  })

  it('counts a live call from the answer, and a settled call to its end', () => {
    expect(callDuration(call({ answered_at: 2_000 }), 12_000)).toBe(10_000)
    expect(
      callDuration(
        call({ state: 'ended', answered_at: 2_000, ended_at: 9_000 }),
        99_000,
      ),
    ).toBe(7_000)
  })

  it('counts an unanswered call from when it was placed', () => {
    expect(
      callDuration(
        call({ state: 'ended', answered_at: null, ended_at: 6_000 }),
        99_000,
      ),
    ).toBe(5_000)
  })
})

describe('the two settled answers', () => {
  it('keeps the outcome apart from the reason the call ended', () => {
    const settled = call({
      state: 'ended',
      outcome: 'answered',
      ended_reason: 'media_timeout',
    })
    expect(outcomeText(settled)).toBe('Answered')
    expect(endedReason(settled.ended_reason)).toBe('the audio stopped')
  })

  it('says whether a message was left on a voicemail', () => {
    expect(outcomeText(call({ outcome: 'voicemail', message_left: true }))).toBe(
      'Voicemail — message left',
    )
    expect(outcomeText(call({ outcome: 'voicemail', message_left: false }))).toBe(
      'Voicemail — no message',
    )
  })

  it('shows a reason this build does not know as the daemon wrote it', () => {
    expect(endedReason('carrier_reset')).toBe('carrier_reset')
    expect(endedReason(null)).toBe('an unstated reason')
  })

  it('names the classification of an outbound call', () => {
    expect(classificationText(call({ classification: 'machine-ivr' }))).toBe(
      'a phone tree answered',
    )
    expect(classificationText(call({ classification: null }))).toBe(
      'nothing classified the answer',
    )
  })

  it('says nothing about the classification of an inbound call', () => {
    // The Agent answered it, so nothing classified the answer and
    // there is no verdict to report (ADR-0020).
    expect(classificationText(call({ direction: 'inbound', classification: null }))).toBe(
      null,
    )
  })
})

describe('who is on the call', () => {
  it('names the direction', () => {
    expect(whoLine(call())).toBe('Call to +1 415 555 9981')
    expect(whoLine(call({ direction: 'inbound' }))).toBe(
      'Call from +1 415 555 9981',
    )
  })

  it('labels a line by its speaker', () => {
    expect(speakerLabel('agent', 'Robin')).toBe('Robin')
    expect(speakerLabel('caller', 'Robin')).toBe('Them')
    expect(speakerLabel('daemon', 'Robin')).toBe('')
  })
})

describe('the transcript', () => {
  const recorded = [
    { at: 1, speaker: 'agent', text: 'Hello.' },
    { at: 2, speaker: 'caller', text: 'Hello there.' },
  ]

  it('adds the lines that arrived after the record was read', () => {
    const merged = mergeTranscript(recorded, [
      { at: 3, speaker: 'agent', text: 'Can we move it?' },
    ])
    expect(merged.map((line) => line.text)).toEqual([
      'Hello.',
      'Hello there.',
      'Can we move it?',
    ])
  })

  it('keeps a line held by both the record and the events once', () => {
    const merged = mergeTranscript(recorded, [
      { at: 2, speaker: 'caller', text: 'Hello there.' },
    ])
    expect(merged).toHaveLength(2)
  })

  it('orders the lines by the time they were said', () => {
    const merged = mergeTranscript(recorded, [
      { at: 0, speaker: 'daemon', text: 'Unknown caller.' },
    ])
    expect(merged[0].text).toBe('Unknown caller.')
  })

  it('takes the last thing said, and never a daemon line', () => {
    expect(
      lastSaid([
        { at: 1, speaker: 'caller', text: 'Wednesday works.' },
        { at: 2, speaker: 'daemon', text: 'The tier fell to Unknown.' },
      ]),
    ).toBe('Wednesday works.')
  })

  it('has nothing to say before anybody speaks', () => {
    expect(lastSaid([])).toBeNull()
    expect(lastSaid([{ at: 1, speaker: 'daemon', text: 'Dialing.' }])).toBeNull()
  })
})


describe('the state chip', () => {
  it('reads Ringing until the call is answered', () => {
    expect(callStateChip(call({ answered_at: null })).label).toBe('Ringing')
  })

  it('reads Connected in the on-call hue while the call runs', () => {
    expect(callStateChip(call())).toEqual({ label: 'Connected', tone: 'on-call' })
  })

  it('reads Ended on a call that ran and finished', () => {
    expect(
      callStateChip(call({ state: 'ended', outcome: 'answered' })),
    ).toEqual({ label: 'Ended', tone: 'neutral' })
  })

  it('reads Failed in the failed hue on a call the carrier refused', () => {
    const refused = call({ state: 'ended', outcome: 'failed', ended_reason: 'refused' })
    expect(callFailed(refused)).toBe(true)
    expect(callStateChip(refused)).toEqual({ label: 'Failed', tone: 'failed' })
  })

  it('reads Failed when the audio never started, whatever the outcome says', () => {
    expect(
      callStateChip(call({ state: 'ended', ended_reason: 'media_failed' })).label,
    ).toBe('Failed')
  })

  it('does not call an unanswered call a failure', () => {
    expect(
      callFailed(call({ state: 'ended', outcome: 'no_answer', ended_reason: 'no_answer' })),
    ).toBe(false)
  })
})

describe('failureText', () => {
  it('says why the call never went through, in plain words', () => {
    expect(failureText(call({ state: 'ended', ended_reason: 'refused' }))).toBe(
      'This call did not go through: the carrier refused the call.',
    )
  })

  it('says the reason is unstated when the daemon gave none', () => {
    expect(failureText(call({ state: 'ended' }))).toBe(
      'This call did not go through: an unstated reason.',
    )
  })
})
