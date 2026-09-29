// The Badge carries one of six tones: gray for a neutral state,
// green for working, amber for waiting, blue for on call, red for
// failed, and the accent.

import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { Badge } from './badge'
import type { BadgeTone } from './badge'

const tones: BadgeTone[] = ['neutral', 'working', 'waiting', 'on-call', 'failed', 'accent']

describe('Badge', () => {
  for (const tone of tones) {
    it(`carries the ${tone} tone in its class`, () => {
      render(<Badge tone={tone}>{tone}</Badge>)
      expect(screen.getByText(tone).className).toContain(`ui-badge-${tone}`)
    })
  }

  it('is neutral without a tone', () => {
    render(<Badge>Never used</Badge>)
    expect(screen.getByText('Never used').className).toContain('ui-badge-neutral')
  })
})
