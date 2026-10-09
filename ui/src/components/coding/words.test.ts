// The words the Product App shows for a Coding Session.

import { describe, expect, it } from 'vitest'

import { ACTIVITIES, COMPUTER_STATES, activityWord, computerStateWord } from '../stateWords'
import {
  answerText,
  approvalModeBadge,
  decisionText,
  endReasonText,
  pendingText,
  planStatusWord,
  sessionStateBadge,
  toolKindWord,
  toolStatusBadge,
  turnEndText,
  sessionSettled,
  usageText,
} from './words'

describe('the state of a session', () => {
  it.each([
    ['starting', 'Opening', 'neutral'],
    ['working', 'Running', 'working'],
    ['needs_decision', 'Waits for a decision', 'waiting'],
    ['idle', 'Ready', 'neutral'],
    ['interrupted', 'Interrupted', 'waiting'],
    ['closed', 'Closed', 'neutral'],
    ['failed', 'Failed', 'failed'],
  ])('reads %s as %s', (state, label, tone) => {
    expect(sessionStateBadge(state)).toEqual({ label, tone })
  })

  it('shows a state that it does not know as the daemon writes it', () => {
    expect(sessionStateBadge('resuming')).toEqual({ label: 'resuming', tone: 'neutral' })
  })

  // A session shows beside the sprite's face, so its words stay apart
  // from the Activity and the Computer state (docs/UI-DESIGN.md).
  it('uses no Activity word and no Computer state word', () => {
    const taken = new Set([
      ...ACTIVITIES.map(activityWord),
      ...COMPUTER_STATES.map((state) => computerStateWord(state, 0)),
    ])
    for (const state of [
      'starting',
      'working',
      'needs_decision',
      'idle',
      'interrupted',
      'closed',
      'failed',
    ]) {
      const { label } = sessionStateBadge(state)
      expect(taken.has(label), `${label} is a word of the sprite`).toBe(false)
    }
  })
})

describe('the Session Approval Mode', () => {
  it('says who approves', () => {
    expect(approvalModeBadge('person', 'Sage')).toEqual({ label: 'You approve', tone: 'neutral' })
    expect(approvalModeBadge('agent', 'Sage')).toEqual({ label: 'Sage approves', tone: 'neutral' })
    expect(approvalModeBadge('auto', 'Sage')).toEqual({
      label: 'Approves everything',
      tone: 'waiting',
    })
  })
})

describe('the usage', () => {
  it('gives the share of the context and the cost', () => {
    expect(
      usageText({
        context_used: 76_000,
        context_size: 200_000,
        cost_amount: 1.5,
        cost_currency: 'USD',
      }),
    ).toBe(`38% of context · ${new Intl.NumberFormat(undefined, { style: 'currency', currency: 'USD' }).format(1.5)}`)
  })

  it('leaves out a part that the harness does not report', () => {
    expect(usageText({ context_used: 50, context_size: 200 })).toBe('25% of context')
    expect(usageText({ cost_amount: 2, cost_currency: 'EUR' })).toBe(
      new Intl.NumberFormat(undefined, { style: 'currency', currency: 'EUR' }).format(2),
    )
    expect(usageText({ context_used: 50, cost_amount: 2 })).toBeNull()
    expect(usageText({})).toBeNull()
  })

  it('shows a currency that it cannot format as the harness writes it', () => {
    expect(usageText({ cost_amount: 0.25, cost_currency: 'credits' })).toBe('0.25 credits')
  })
})

describe('a tool call', () => {
  it.each([
    ['pending', 'Waiting', 'neutral'],
    ['in_progress', 'Running', 'working'],
    ['completed', 'Done', 'neutral'],
    ['failed', 'Failed', 'failed'],
  ])('reads the status %s as %s', (status, label, tone) => {
    expect(toolStatusBadge(status)).toEqual({ label, tone })
  })

  it('names its kind in words', () => {
    expect(toolKindWord('execute')).toBe('Command')
    expect(toolKindWord('edit')).toBe('Edit')
    expect(toolKindWord(null)).toBe('Tool')
  })
})

describe('the end of a turn', () => {
  it.each([
    ['end_turn', 'The turn ended'],
    ['cancelled', 'The turn was stopped'],
    ['max_tokens', 'The turn ran out of tokens'],
    ['max_turn_requests', 'The turn reached its request limit'],
    ['refusal', 'The harness refused the turn'],
  ])('reads %s as %s', (reason, text) => {
    expect(turnEndText(reason)).toBe(text)
  })
})

describe('the decision on a Harness Permission', () => {
  it('names the policy that allowed it', () => {
    expect(decisionText('allow_once', 'auto', 'Sage')).toBe(
      'Allowed: the session allows everything',
    )
    expect(decisionText('allow_once', 'scope', 'Sage')).toBe(
      "Allowed: inside the session's directory",
    )
    expect(decisionText('allow_once', 'rule', 'Sage')).toBe('Allowed by a rule')
  })

  it('names the sprite or the Person that answered', () => {
    expect(decisionText('allow_once', 'agent', 'Sage')).toBe('Sage allowed it')
    expect(decisionText('reject_once', 'agent', 'Sage')).toBe('Sage denied it')
    expect(decisionText('allow_once', 'person', 'Sage')).toBe('You allowed it')
    expect(decisionText('reject_once', 'person', 'Sage')).toBe('You denied it')
  })

  it('reads a request that a cancel ended as cancelled with the turn', () => {
    expect(decisionText('withdrawn', null, 'Sage')).toBe('Cancelled with the turn')
  })

  // Pagis answers `cancelled` when the harness offers no option to
  // allow once.
  it('reads a request that Pagis answered with a cancel as cancelled', () => {
    expect(decisionText('cancel', 'scope', 'Sage')).toBe('Cancelled')
  })
})

describe('the answer to a question', () => {
  it('says how the question ended', () => {
    expect(answerText({ accept: { branch: 'main' } })).toBe('Answered')
    expect(answerText('decline')).toBe('Declined')
    expect(answerText('cancel')).toBe('Cancelled')
    expect(answerText('withdrawn')).toBe('Cancelled with the turn')
  })
})

describe('a plan entry', () => {
  it('says its status', () => {
    expect(planStatusWord('pending')).toBe('Not started')
    expect(planStatusWord('in_progress')).toBe('In progress')
    expect(planStatusWord('completed')).toBe('Done')
  })
})

describe('the end of a session', () => {
  it.each([
    ['closed', 'Sage closed the session'],
    ['stopped', 'You stopped the session'],
    ['harness_error', 'The coding harness failed a request'],
    ['harness_exited', 'The coding harness stopped by itself'],
    ['temporarily_unavailable', 'Pagis could not follow the session'],
    ['not_found', 'The coding harness is not installed on the machine'],
    ['bad_directory', 'The directory does not exist'],
    ['worktree_failed', 'Pagis could not make the worktree'],
    ['spawn_failed', 'The coding harness did not start'],
    ['host_not_connected', 'The machine was not connected'],
  ])('says why %s ended it', (reason, text) => {
    expect(endReasonText(reason, 'Sage')).toBe(text)
  })

  it('shows a reason that it does not know as the daemon writes it', () => {
    expect(endReasonText('quota_gone', 'Sage')).toBe('quota_gone')
  })

  it('says the session ended when the daemon gives no reason', () => {
    expect(endReasonText(null, 'Sage')).toBe('The session ended')
  })

  it('settles in the terminal states only', () => {
    expect(sessionSettled('closed')).toBe(true)
    expect(sessionSettled('failed')).toBe(true)
    for (const state of ['starting', 'working', 'needs_decision', 'idle', 'interrupted']) {
      expect(sessionSettled(state)).toBe(false)
    }
  })
})

describe('a decision that waits', () => {
  it('says where it waits', () => {
    expect(pendingText({ kind: 'permission', waits_for: 'person', seq: 1 }, 'Sage', 'Codex')).toBe(
      'A permission waits for your answer in this thread.',
    )
    expect(pendingText({ kind: 'permission', waits_for: 'agent', seq: 1 }, 'Sage', 'Codex')).toBe(
      'Sage decides a permission.',
    )
    expect(pendingText({ kind: 'question', waits_for: 'agent', seq: 1 }, 'Sage', 'Codex')).toBe(
      'Sage answers a question from Codex.',
    )
  })
})
