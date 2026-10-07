// The gestures of the live screen, as a pure state machine over pointer
// events. It follows the touch mode of remote desktop clients such as
// Microsoft Remote Desktop: a tap clicks, a hold clicks with the right
// button, and two fingers zoom and pan the view. A mouse and a pen point
// as they do on the Computer.
//
// The outputs carry client points and client distances. The live screen
// maps them to capture pixels, and drops the input outputs when the
// Person does not hold the switch.

import type { Point, ViewChange } from './screenView'

/** A finger that holds still this long clicks with the right button. */
const HOLD_MS = 500
/** A finger that moves less than this is still. */
const SLOP_PX = 10
/** The longest gap between the two taps of a double tap. */
const DOUBLE_TAP_MS = 300

export type ScreenButton = 'left' | 'middle' | 'right'

export interface GestureEvent {
  type: 'down' | 'move' | 'up' | 'cancel'
  id: number
  /** `touch`, or a mouse or a pen. */
  pointerType: string
  point: Point
  /** Milliseconds, from the event's time stamp. */
  time: number
  /** The button of a mouse or a pen: 0 is left, 1 is middle, 2 is right. */
  button: number
}

export type GestureOutput =
  | { kind: 'move'; point: Point }
  | { kind: 'button'; button: ScreenButton; down: boolean }
  /** A wheel scroll in client pixels: positive is right and down. */
  | { kind: 'scroll'; dx: number; dy: number }
  | { kind: 'view'; change: ViewChange }

interface Finger {
  id: number
  start: Point
  point: Point
}

/** What the fingers do now. */
type Touch =
  | { mode: 'idle' }
  /** One finger that is still: a tap, a hold or the start of a drag. */
  | { mode: 'press'; finger: Finger; since: number }
  | { mode: 'scroll'; finger: Finger }
  | { mode: 'drag'; finger: Finger }
  /** Two fingers zoom and pan. `tap` stays true while they are still. */
  | { mode: 'pinch'; fingers: Finger[]; since: number; tap: boolean }

export interface GestureState {
  touch: Touch
  /** The buttons that are down on the Computer. */
  held: ScreenButton[]
  /** When the last tap with two fingers ended. */
  lastTwoFingerTap: number | null
}

export const initialGestureState: GestureState = {
  touch: { mode: 'idle' },
  held: [],
  lastTwoFingerTap: null,
}

type Step = [GestureState, GestureOutput[]]

const distance = (a: Point, b: Point) => Math.hypot(a.x - b.x, a.y - b.y)
const midpoint = (a: Point, b: Point): Point => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 })

function mouseButton(button: number): ScreenButton {
  return button === 2 ? 'right' : button === 1 ? 'middle' : 'left'
}

function press(state: GestureState, button: ScreenButton): Step {
  return [
    { ...state, held: [...state.held.filter((held) => held !== button), button] },
    [{ kind: 'button', button, down: true }],
  ]
}

function release(state: GestureState, button: ScreenButton): Step {
  return [
    { ...state, held: state.held.filter((held) => held !== button) },
    [{ kind: 'button', button, down: false }],
  ]
}

/** Every button that is down goes up. */
function releaseAll(state: GestureState): Step {
  return [
    { ...state, held: [] },
    state.held.map((button) => ({ kind: 'button', button, down: false })),
  ]
}

function click(state: GestureState, at: Point, button: ScreenButton): Step {
  const [pressed, down] = press(state, button)
  const [released, up] = release(pressed, button)
  return [released, [{ kind: 'move', point: at }, ...down, ...up]]
}

function stepMouse(state: GestureState, event: GestureEvent): Step {
  const button = mouseButton(event.button)
  switch (event.type) {
    case 'move':
      return [state, [{ kind: 'move', point: event.point }]]
    case 'down': {
      const [next, outputs] = press(state, button)
      return [next, [{ kind: 'move', point: event.point }, ...outputs]]
    }
    case 'up':
      return release(state, button)
    case 'cancel':
      return releaseAll(state)
  }
}

/** A second finger starts a pinch. A drag lets its button go first. */
function startPinch(state: GestureState, first: Finger, event: GestureEvent, since: number): Step {
  const [released, outputs] = releaseAll(state)
  const second = { id: event.id, start: event.point, point: event.point }
  return [
    { ...released, touch: { mode: 'pinch', fingers: [first, second], since, tap: true } },
    outputs,
  ]
}

/** The view changes of one finger's move in a pinch: a zoom about the
 * old midpoint by the change in spread, then a pan by the move of the
 * midpoint. */
function pinchMove(fingers: Finger[], moved: Finger[]): GestureOutput[] {
  const [a, b] = fingers
  const [c, d] = moved
  const before = midpoint(a.point, b.point)
  const after = midpoint(c.point, d.point)
  const spread = distance(a.point, b.point)
  const outputs: GestureOutput[] = []
  if (spread > 0) {
    outputs.push({
      kind: 'view',
      change: { type: 'zoom', factor: distance(c.point, d.point) / spread, about: before },
    })
  }
  outputs.push({
    kind: 'view', change: { type: 'pan', dx: after.x - before.x, dy: after.y - before.y },
  })
  return outputs
}

/** The last finger of a pinch went up. Two still taps close together
 * set the zoom back to 1x. */
function endPinch(state: GestureState, touch: Extract<Touch, { mode: 'pinch' }>, time: number): Step {
  const idle: GestureState = { ...state, touch: { mode: 'idle' } }
  if (!touch.tap || time - touch.since >= HOLD_MS) {
    return [{ ...idle, lastTwoFingerTap: null }, []]
  }
  const last = state.lastTwoFingerTap
  if (last !== null && touch.since - last <= DOUBLE_TAP_MS) {
    return [{ ...idle, lastTwoFingerTap: null }, [{ kind: 'view', change: { type: 'reset' } }]]
  }
  return [{ ...idle, lastTwoFingerTap: time }, []]
}

function stepPinch(
  state: GestureState, touch: Extract<Touch, { mode: 'pinch' }>, event: GestureEvent,
): Step {
  const index = touch.fingers.findIndex((finger) => finger.id === event.id)
  // A third finger takes no part.
  if (index === -1) return [state, []]
  if (event.type === 'move') {
    const fingers = touch.fingers.map((finger, at) =>
      at === index ? { ...finger, point: event.point } : finger)
    const still = distance(fingers[index].start, event.point) <= SLOP_PX
    return [
      { ...state, touch: { ...touch, fingers, tap: touch.tap && still } },
      fingers.length === 2 ? pinchMove(touch.fingers, fingers) : [],
    ]
  }
  // A finger went up or the browser took it. The pinch ends with its
  // last finger, and the one that stays does not drive the Computer.
  const fingers = touch.fingers.filter((_, at) => at !== index)
  const next = { ...touch, fingers, tap: touch.tap && event.type === 'up' }
  if (fingers.length > 0) return [{ ...state, touch: next }, []]
  return endPinch(state, next, event.time)
}

function stepTouch(state: GestureState, event: GestureEvent): Step {
  const touch = state.touch
  const idle: GestureState = { ...state, touch: { mode: 'idle' } }
  if (touch.mode === 'idle') {
    if (event.type !== 'down') return [state, []]
    const finger = { id: event.id, start: event.point, point: event.point }
    return [{ ...state, touch: { mode: 'press', finger, since: event.time } }, []]
  }
  if (touch.mode === 'pinch') return stepPinch(state, touch, event)

  const finger = touch.finger
  if (event.type === 'down') {
    return startPinch(state, finger, event, touch.mode === 'press' ? touch.since : event.time)
  }
  // Another finger of a gesture that ignores it.
  if (event.id !== finger.id) return [state, []]
  const moved = { ...finger, point: event.point }

  switch (touch.mode) {
    case 'press': {
      const held = event.time - touch.since >= HOLD_MS
      if (event.type === 'up') return click(idle, finger.start, held ? 'right' : 'left')
      if (event.type === 'cancel') return [idle, []]
      if (distance(finger.start, event.point) <= SLOP_PX) {
        return [{ ...state, touch: { ...touch, finger: moved } }, []]
      }
      if (held) {
        const [pressed, down] = press(state, 'left')
        return [
          { ...pressed, touch: { mode: 'drag', finger: moved } },
          [{ kind: 'move', point: finger.start }, ...down, { kind: 'move', point: event.point }],
        ]
      }
      return scroll(state, finger, moved)
    }
    case 'scroll':
      if (event.type === 'move') return scroll(state, finger, moved)
      return [idle, []]
    case 'drag':
      if (event.type === 'move') {
        return [{ ...state, touch: { mode: 'drag', finger: moved } }, [{ kind: 'move', point: event.point }]]
      }
      return releaseAll(idle)
  }
}

/** A drag before the hold scrolls the page under the finger, as a page
 * scrolls under a finger on a phone: a move up scrolls down. */
function scroll(state: GestureState, finger: Finger, moved: Finger): Step {
  return [
    { ...state, touch: { mode: 'scroll', finger: moved } },
    [{
      kind: 'scroll',
      dx: finger.point.x - moved.point.x,
      dy: finger.point.y - moved.point.y,
    }],
  ]
}

/** The next state and the outputs of one pointer event. */
export function stepGestures(state: GestureState, event: GestureEvent): Step {
  return event.pointerType === 'touch' ? stepTouch(state, event) : stepMouse(state, event)
}
