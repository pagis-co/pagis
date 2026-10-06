// The geometry of the live screen. The video fills its frame with
// `object-fit: contain`, so the capture shows in a content box between
// bars. The view zooms and pans the video with a CSS transform,
// `translate(x, y) scale(scale)` about the top left of the element.
// Every function is pure, so the tests need no layout.

export interface Point {
  x: number
  y: number
}

export interface Size {
  width: number
  height: number
}

/** A box in client pixels, or in the pixels of the element. */
export interface Rect extends Size {
  left: number
  top: number
}

/** The zoom and the pan of the video, in client pixels. */
export interface View {
  scale: number
  x: number
  y: number
}

/** A change that a gesture makes to the view. Points are client points. */
export type ViewChange =
  | { type: 'zoom'; factor: number; about: Point }
  | { type: 'pan'; dx: number; dy: number }
  | { type: 'reset' }

/** The capture surface, fixed by the pipeline. */
export const CAPTURE: Size = { width: 1280, height: 800 }

export const IDENTITY_VIEW: View = { scale: 1, x: 0, y: 0 }

const MIN_SCALE = 1
const MAX_SCALE = 4

/** The box of the media inside an element with `object-fit: contain`. */
export function contentBox(element: Size, media: Size): Rect {
  const fit = Math.min(element.width / media.width, element.height / media.height)
  const width = media.width * fit
  const height = media.height * fit
  return {
    left: (element.width - width) / 2,
    top: (element.height - height) / 2,
    width,
    height,
  }
}

const clamp = (value: number, low: number, high: number) =>
  Math.min(Math.max(value, low), high)

/** The capture pixel under a client point. A point over a bar gives the
 * nearest pixel on the edge of the capture. */
export function capturePoint(point: Point, box: Rect, view: View): Point {
  const content = contentBox(box, CAPTURE)
  // The point in the element before the transform.
  const x = (point.x - box.left - view.x) / view.scale
  const y = (point.y - box.top - view.y) / view.scale
  return {
    x: clamp(((x - content.left) / content.width) * CAPTURE.width, 0, CAPTURE.width - 1),
    y: clamp(((y - content.top) / content.height) * CAPTURE.height, 0, CAPTURE.height - 1),
  }
}

/** The capture pixels under one client pixel. */
export function capturePerPixel(box: Size, view: View): number {
  return CAPTURE.width / (contentBox(box, CAPTURE).width * view.scale)
}

/** The translation on one axis. Where the zoomed content is larger than
 * the frame, it covers the frame. Where it is smaller, it is at the center. */
function clampAxis(
  translation: number, scale: number, frame: number, start: number, length: number,
): number {
  const shown = length * scale
  if (shown <= frame) return (frame - shown) / 2 - start * scale
  // `0 -` and not a unary minus, so a content box at the edge gives 0 and not -0.
  return clamp(translation, frame - (start + length) * scale, 0 - start * scale)
}

/** The view with its zoom from 1x to 4x and its pan inside the content. */
export function clampView(view: View, box: Size): View {
  const scale = clamp(view.scale, MIN_SCALE, MAX_SCALE)
  const content = contentBox(box, CAPTURE)
  return {
    scale,
    x: clampAxis(view.x, scale, box.width, content.left, content.width),
    y: clampAxis(view.y, scale, box.height, content.top, content.height),
  }
}

/** The view after a change, clamped. */
export function applyViewChange(view: View, change: ViewChange, box: Rect): View {
  switch (change.type) {
    case 'reset':
      return IDENTITY_VIEW
    case 'pan':
      return clampView({ ...view, x: view.x + change.dx, y: view.y + change.dy }, box)
    case 'zoom': {
      // The element point under `about` stays under it.
      const scale = clamp(view.scale * change.factor, MIN_SCALE, MAX_SCALE)
      const ratio = scale / view.scale
      const aboutX = change.about.x - box.left
      const aboutY = change.about.y - box.top
      return clampView(
        { scale, x: aboutX - (aboutX - view.x) * ratio, y: aboutY - (aboutY - view.y) * ratio },
        box,
      )
    }
  }
}
