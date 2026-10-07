// The gestures of the live screen: the pointer events of a mouse, a pen
// and the fingers of a touch screen, and the input ops and view changes
// that they give.

import { describe, expect, it } from 'vitest'

import {
  type GestureEvent,
  type GestureOutput,
  initialGestureState,
  stepGestures,
} from './screenGestures'
import { type View, applyViewChange } from './screenView'

/** A frame of the capture's own aspect: no bars. */
const BOX = { left: 0, top: 0, width: 1280, height: 800 }

function touch(
  type: GestureEvent['type'], id: number, x: number, y: number, time: number,
): GestureEvent {
  return { type, id, pointerType: 'touch', point: { x, y }, time, button: 0 }
}

function mouse(
  type: GestureEvent['type'], x: number, y: number, button = 0, pointerType = 'mouse',
): GestureEvent {
  return { type, id: 1, pointerType, point: { x, y }, time: 0, button }
}

/** The input ops that the events give, and the view after their view
 * changes. */
function run(events: GestureEvent[], view: View = { scale: 1, x: 0, y: 0 }) {
  let state = initialGestureState
  const inputs: GestureOutput[] = []
  for (const event of events) {
    const [next, outputs] = stepGestures(state, event)
    state = next
    for (const output of outputs) {
      if (output.kind === 'view') view = applyViewChange(view, output.change, BOX)
      else inputs.push(output)
    }
  }
  return { inputs, view }
}

const move = (x: number, y: number): GestureOutput => ({ kind: 'move', point: { x, y } })
const button = (name: 'left' | 'right' | 'middle', down: boolean): GestureOutput =>
  ({ kind: 'button', button: name, down })

/** Two fingers that start 200 px apart about (500, 400) and spread to
 * `spread` px apart about the same point. */
function pinch(spread: number, start = 0): GestureEvent[] {
  return [
    touch('down', 1, 400, 400, start),
    touch('down', 2, 600, 400, start + 10),
    touch('move', 1, 500 - spread / 2, 400, start + 50),
    touch('move', 2, 500 + spread / 2, 400, start + 60),
    touch('up', 1, 500 - spread / 2, 400, start + 100),
    touch('up', 2, 500 + spread / 2, 400, start + 110),
  ]
}

/** A tap with two fingers that are still. */
function twoFingerTap(start: number): GestureEvent[] {
  return [
    touch('down', 3, 300, 300, start),
    touch('down', 4, 400, 300, start + 10),
    touch('up', 3, 300, 300, start + 80),
    touch('up', 4, 400, 300, start + 90),
  ]
}

describe('a mouse and a pen', () => {
  it('move the pointer and press the button that the event names', () => {
    const { inputs } = run([
      mouse('move', 10, 20),
      mouse('down', 30, 40, 2),
      mouse('up', 30, 40, 2),
      mouse('down', 30, 40, 1, 'pen'),
      mouse('up', 30, 40, 1, 'pen'),
    ])
    expect(inputs).toEqual([
      move(10, 20),
      move(30, 40), button('right', true),
      button('right', false),
      move(30, 40), button('middle', true),
      button('middle', false),
    ])
  })
})

describe('one finger', () => {
  it('taps as a left click at the point where it went down', () => {
    const { inputs } = run([
      touch('down', 1, 100, 100, 0),
      touch('move', 1, 103, 101, 40),
      touch('up', 1, 103, 101, 120),
    ])
    expect(inputs).toEqual([move(100, 100), button('left', true), button('left', false)])
  })

  it('holds for 500 ms as a right click', () => {
    const { inputs } = run([
      touch('down', 1, 100, 100, 0),
      touch('move', 1, 104, 102, 300),
      touch('up', 1, 104, 102, 600),
    ])
    expect(inputs).toEqual([move(100, 100), button('right', true), button('right', false)])
  })

  it('drags with the left button down after the hold', () => {
    const { inputs } = run([
      touch('down', 1, 100, 100, 0),
      touch('move', 1, 150, 100, 600),
      touch('move', 1, 200, 120, 650),
      touch('up', 1, 200, 120, 700),
    ])
    expect(inputs).toEqual([
      move(100, 100), button('left', true), move(150, 100),
      move(200, 120),
      button('left', false),
    ])
  })

  it('scrolls the page with a drag before the hold, as the finger moves it', () => {
    const { inputs } = run([
      touch('down', 1, 100, 300, 0),
      touch('move', 1, 100, 250, 100),
      touch('move', 1, 90, 200, 150),
      touch('up', 1, 90, 200, 200),
    ])
    expect(inputs).toEqual([
      { kind: 'scroll', dx: 0, dy: 50 },
      { kind: 'scroll', dx: 10, dy: 50 },
    ])
  })
})

describe('two fingers', () => {
  it('pinch to 2× about their midpoint and send no input', () => {
    const { inputs, view } = run(pinch(400))
    expect(inputs).toEqual([])
    // The point under the midpoint (500, 400) stays under it.
    expect(view.scale).toBeCloseTo(2)
    expect(view.x).toBeCloseTo(500 - 2 * 500)
    expect(view.y).toBeCloseTo(400 - 2 * 400)
  })

  it('zoom no further than 4×', () => {
    const { view } = run(pinch(1200))
    expect(view.scale).toBe(4)
  })

  it('pan the zoomed view when they drag together, and send no input', () => {
    const { inputs, view } = run([
      touch('down', 1, 400, 400, 0),
      touch('down', 2, 600, 400, 10),
      touch('move', 1, 300, 350, 50),
      touch('move', 2, 500, 350, 60),
      touch('up', 1, 300, 350, 100),
      touch('up', 2, 500, 350, 110),
    ], { scale: 2, x: -640, y: -400 })
    expect(inputs).toEqual([])
    expect(view.scale).toBeCloseTo(2)
    expect(view.x).toBeCloseTo(-740)
    expect(view.y).toBeCloseTo(-450)
  })

  it('set the zoom back to 1× with a double tap', () => {
    const zoomed = { scale: 2, x: -640, y: -400 }
    expect(run(twoFingerTap(0), zoomed).view).toEqual(zoomed)

    const { inputs, view } = run([...twoFingerTap(0), ...twoFingerTap(250)], zoomed)

    expect(inputs).toEqual([])
    expect(view).toEqual({ scale: 1, x: 0, y: 0 })
  })

  it('release the button of a drag when the second finger goes down', () => {
    const { inputs } = run([
      touch('down', 1, 100, 100, 0),
      touch('move', 1, 150, 100, 600),
      touch('down', 2, 300, 100, 650),
    ])
    expect(inputs.at(-1)).toEqual(button('left', false))
  })
})

describe('pointercancel', () => {
  it('releases the button of a drag', () => {
    const { inputs } = run([
      touch('down', 1, 100, 100, 0),
      touch('move', 1, 150, 100, 600),
      touch('cancel', 1, 150, 100, 650),
      touch('up', 1, 150, 100, 700),
    ])
    expect(inputs).toEqual([
      move(100, 100), button('left', true), move(150, 100),
      button('left', false),
    ])
  })

  it('releases a mouse button that is down', () => {
    const { inputs } = run([mouse('down', 30, 40, 0), mouse('cancel', 30, 40)])
    expect(inputs).toEqual([move(30, 40), button('left', true), button('left', false)])
  })

  it('sends nothing for a finger that clicked nothing yet', () => {
    const { inputs } = run([
      touch('down', 1, 100, 100, 0),
      touch('cancel', 1, 100, 100, 50),
    ])
    expect(inputs).toEqual([])
  })
})
