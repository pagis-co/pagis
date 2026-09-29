import { describe, expect, it } from 'vitest'
import { avatarMotion } from './motion'

describe('avatar motion rules', () => {
  it('uses presence without letting hover hide work or a request for input', () => {
    expect(
      avatarMotion({ presence: 'working', online: true, hover: true }),
    ).toEqual({ clip: 'Working', expression: 'Neutral', animate: true })
    expect(
      avatarMotion({ presence: 'waiting', online: true, hover: true }).clip,
    ).toBe('NeedsInput')
    expect(
      avatarMotion({ presence: 'idle', online: true, hover: true }),
    ).toEqual({ clip: 'Celebrate', expression: 'Smile', animate: true })
    expect(avatarMotion({ presence: 'idle', online: true }).clip).toBe(
      'Waiting',
    )
    expect(avatarMotion({ presence: 'oncall', online: true }).clip).toBe('Idle')
    expect(avatarMotion({ presence: 'working', online: false }).animate).toBe(
      false,
    )
    expect(
      avatarMotion({ presence: 'working', online: true, reducedMotion: true })
        .animate,
    ).toBe(false)
  })
})

import { createAvatarReactions } from './reactions'
import type { EventRow } from '../api/client'
it('reacts once to a live reply in the open conversation and ignores replay and reflection failure', () => {
  const reactions = createAvatarReactions()
  const reply: EventRow = {
    id: 'event-1',
    event_type: 'message.completed',
    agent_id: 'sage',
    channel_id: 'open',
    run_id: 'run',
    payload: { author_kind: 'agent' },
  }
  expect(
    reactions.accept(reply, {
      replay: true,
      channelId: 'open',
      previousRunState: 'running',
    }),
  ).toBeNull()
  expect(
    reactions.accept(
      { ...reply, id: 'event-2' },
      { replay: false, channelId: 'elsewhere', previousRunState: 'running' },
    ),
  ).toBeNull()
  expect(
    reactions.accept(
      { ...reply, id: 'event-3' },
      { replay: false, channelId: 'open', previousRunState: 'running' },
    ),
  ).toEqual({
    agentId: 'sage',
    channelId: 'open',
    expression: 'Smile',
    clip: 'Idle',
    seconds: 1.6,
  })
  expect(
    reactions.accept(
      { ...reply, id: 'event-3' },
      { replay: false, channelId: 'open', previousRunState: 'running' },
    ),
  ).toBeNull()
  const failed = {
    ...reply,
    id: 'error-1',
    event_type: 'run.state_changed',
    payload: { to: 'failed' },
  }
  expect(
    reactions.accept(failed, {
      replay: false,
      channelId: 'open',
      previousRunState: 'reflecting',
    }),
  ).toBeNull()
  expect(
    reactions.accept(
      { ...failed, id: 'error-2' },
      { replay: false, channelId: 'open', previousRunState: 'running' },
    )?.clip,
  ).toBe('Error')
  expect(
    reactions.accept(
      { ...failed, id: 'cancel-1', payload: { to: 'canceled' } },
      { replay: false, channelId: 'open', previousRunState: 'running' },
    ),
  ).toBeNull()
})
