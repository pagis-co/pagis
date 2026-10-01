import { render, waitFor } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'

import { defaultAppearance } from './catalog'
import SpriteScene from './SpriteScene'

// jsdom has no WebGL. The renderer records the drawing size the scene
// asks for, which is the size of the live view in the disc.
const drawn: { width: number; height: number }[] = []
vi.mock('three', async (actual) => ({
  ...(await actual<typeof import('three')>()),
  WebGLRenderer: class {
    domElement: HTMLCanvasElement
    constructor({ canvas }: { canvas: HTMLCanvasElement }) {
      this.domElement = canvas
    }
    setPixelRatio() {}
    setSize(width: number, height: number) {
      drawn.push({ width, height })
    }
    render() {}
    dispose() {}
  },
}))
// The model never loads, so the scene draws an empty stage.
vi.mock('./rendering', () => ({ loadSprite: () => new Promise(() => {}) }))

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  drawn.length = 0
})

it('draws the live view at the size of the disc that a transform frames', async () => {
  // The disc frames the figure with a CSS scale, so the box the browser
  // reports for anything inside it is larger than its layout size.
  // jsdom applies no CSS, so the test gives each element both sizes.
  const scale = 1.3
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(28)
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(28)
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(
    DOMRect.fromRect({ width: 28 * scale, height: 28 * scale }),
  )
  // The page lays the disc out once, and the scene measures it then.
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(private readonly measured: () => void) {}
      observe() {
        this.measured()
      }
      disconnect() {}
    },
  )

  const view = render(
    <SpriteScene
      onReady={() => {}}
      appearance={defaultAppearance()}
      clip="Idle"
    />,
  )

  await waitFor(() => expect(drawn.at(-1)).toEqual({ width: 28, height: 28 }))
  view.unmount()
})
