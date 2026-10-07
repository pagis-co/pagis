// The geometry of the live screen: where the video content shows inside
// its element, which capture pixel a client point is over, and how far
// the zoomed view may pan.

import { describe, expect, it } from 'vitest'

import {
  CAPTURE,
  type View,
  applyViewChange,
  capturePerPixel,
  capturePoint,
  clampView,
  contentBox,
} from './screenView'

/** A frame wider than 16:10: bars on the left and the right. The
 * content is 640 × 400 at x 180. */
const WIDE = { left: 0, top: 0, width: 1000, height: 400 }

const IDENTITY: View = { scale: 1, x: 0, y: 0 }

describe('contentBox', () => {
  it('puts bars on the sides of a frame wider than the capture', () => {
    expect(contentBox({ width: 1000, height: 400 }, CAPTURE)).toEqual({
      left: 180, top: 0, width: 640, height: 400,
    })
  })

  it('puts bars on the top and the bottom of a frame taller than the capture', () => {
    expect(contentBox({ width: 400, height: 1000 }, CAPTURE)).toEqual({
      left: 0, top: 375, width: 400, height: 250,
    })
  })
})

describe('capturePoint', () => {
  it('maps a client point through the content box at 1×', () => {
    const box = { left: 10, top: 20, width: 1000, height: 400 }
    expect(capturePoint({ x: 10 + 180 + 160, y: 20 + 100 }, box, IDENTITY))
      .toEqual({ x: 320, y: 200 })
  })

  it('maps a client point through the zoom and the pan at 2×', () => {
    const view = { scale: 2, x: -500, y: -200 }
    // The element point is ((300 + 500) / 2, (100 + 200) / 2) = (400, 150),
    // which is 220 px into the content box of 640 px.
    expect(capturePoint({ x: 300, y: 100 }, WIDE, view)).toEqual({ x: 440, y: 300 })
  })

  it('maps the corners of the frame at 4× to the middle half of the capture', () => {
    const box = { left: 0, top: 0, width: 1280, height: 800 }
    const view = { scale: 4, x: -1920, y: -1200 }
    expect(capturePoint({ x: 0, y: 0 }, box, view)).toEqual({ x: 480, y: 300 })
    expect(capturePoint({ x: 1280, y: 800 }, box, view)).toEqual({ x: 800, y: 500 })
  })

  it('keeps a point over a bar on the edge of the capture', () => {
    expect(capturePoint({ x: 5, y: 399 }, WIDE, IDENTITY)).toEqual({ x: 0, y: 798 })
    expect(capturePoint({ x: 995, y: 0 }, WIDE, IDENTITY)).toEqual({
      x: CAPTURE.width - 1, y: 0,
    })
  })
})

describe('capturePerPixel', () => {
  it('gives the capture pixels under one client pixel', () => {
    // 640 client pixels show 1280 capture pixels at 1×.
    expect(capturePerPixel(WIDE, IDENTITY)).toBe(2)
    expect(capturePerPixel(WIDE, { scale: 4, x: 0, y: 0 })).toBe(0.5)
  })
})

describe('clampView', () => {
  it('keeps the zoomed content over the whole frame', () => {
    // At 2× the content is 1280 × 800 at x 360, and the frame is
    // 1000 × 400, so x stays in [-640, -360] and y in [-400, 0].
    expect(clampView({ scale: 2, x: 0, y: 50 }, WIDE)).toEqual({ scale: 2, x: -360, y: 0 })
    expect(clampView({ scale: 2, x: -2000, y: -2000 }, WIDE))
      .toEqual({ scale: 2, x: -640, y: -400 })
    expect(clampView({ scale: 2, x: -500, y: -200 }, WIDE))
      .toEqual({ scale: 2, x: -500, y: -200 })
  })

  it('centres the content on an axis where it is smaller than the frame', () => {
    // At 1.25× the content is 800 wide in a frame of 1000.
    expect(clampView({ scale: 1.25, x: -400, y: 30 }, WIDE))
      .toEqual({ scale: 1.25, x: -125, y: 0 })
  })

  it('keeps the zoom from 1× to 4×', () => {
    expect(clampView({ scale: 0.5, x: 10, y: 10 }, WIDE)).toEqual(IDENTITY)
    expect(clampView({ scale: 9, x: 0, y: 0 }, WIDE).scale).toBe(4)
  })
})

describe('applyViewChange', () => {
  it('zooms about a client point, which stays over the same capture pixel', () => {
    const box = { left: 100, top: 50, width: 1000, height: 400 }
    const about = { x: 600, y: 250 }
    const before = capturePoint(about, box, IDENTITY)

    const view = applyViewChange(IDENTITY, { type: 'zoom', factor: 2, about }, box)

    expect(view.scale).toBe(2)
    expect(capturePoint(about, box, view)).toEqual(before)
  })

  it('pans by a client distance inside the clamp', () => {
    const view = applyViewChange(
      { scale: 2, x: -500, y: -200 }, { type: 'pan', dx: 1000, dy: -30 }, WIDE,
    )
    expect(view).toEqual({ scale: 2, x: -360, y: -230 })
  })

  it('resets to 1×', () => {
    expect(applyViewChange({ scale: 3, x: -900, y: -500 }, { type: 'reset' }, WIDE))
      .toEqual(IDENTITY)
  })
})
