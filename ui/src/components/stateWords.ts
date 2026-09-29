// The words the Product App shows for the state of one Agent. There
// are two sets, and no word is in both (CONTEXT.md):
//
// - the Activity: what the Agent does now, from its Runs and its
//   Calls. It says nothing about the Computer.
// - the Computer state: whether the Agent's Computer runs. It says
//   nothing about the Agent's work.
//
// Every surface that shows a state takes its word from here.

import type { BadgeTone, Presence } from '../primitives'
import type { Holder } from './ScreenFrame'

/** Each Activity an Agent can have. `none` is not one: it is a face
 *  that shows no Activity. */
export const ACTIVITIES = ['working', 'waiting', 'oncall', 'idle'] as const

const ACTIVITY_WORD: Record<Presence, string> = {
  working: 'Working',
  waiting: 'Needs you',
  oncall: 'On a call',
  idle: 'Idle',
  none: '',
}

const ACTIVITY_TONE: Record<Presence, BadgeTone> = {
  working: 'working',
  waiting: 'waiting',
  oncall: 'on-call',
  idle: 'neutral',
  none: 'neutral',
}

/** The Activity in one word. */
export function activityWord(activity: Presence): string {
  return ACTIVITY_WORD[activity]
}

/** The hue a badge of the Activity wears. */
export function activityTone(activity: Presence): BadgeTone {
  return ACTIVITY_TONE[activity]
}

/** The states the daemon gives a Computer. */
export const COMPUTER_STATES = ['off', 'pulling', 'starting', 'awake', 'failed'] as const

/** The Computer state in the words of the Desk. A state the Product
 *  App does not know reads as the daemon names it. */
export function computerStateWord(
  state: string,
  percent: number | null | undefined,
): string {
  switch (state) {
    case 'off':
      return 'Asleep'
    case 'pulling':
      return `Downloading the computer… ${percent ?? 0}%`
    case 'starting':
      return 'Starting…'
    case 'awake':
      return 'Awake'
    case 'failed':
      return 'Needs attention'
    default:
      return state
  }
}

/** Who sends the input to the screen. The Agent can hold the screen
 *  and run nothing, so these words say control, not work. */
export function controlWord(holder: Holder, agentName: string): string {
  switch (holder) {
    case 'agent':
      return `${agentName} is in control`
    case 'user':
      return 'You are in control'
    case 'daemon':
      return 'Pagis is filling a saved login'
  }
}
