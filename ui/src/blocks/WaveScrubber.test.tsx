// The recording scrubber of the call card: a slider that the
// keyboard can drive, the peaks it draws, and the flat bar it falls
// back to when nothing can decode the audio.

import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { BARS, WaveScrubber, peaksFrom } from './WaveScrubber'

describe('peaksFrom', () => {
  it('draws one bar per slot, loudest at full height', () => {
    const samples = new Float32Array(BARS * 10)
    samples[0] = 0.5
    samples[samples.length - 1] = 1
    const peaks = peaksFrom(samples)
    expect(peaks).toHaveLength(BARS)
    expect(peaks[0]).toBeCloseTo(0.5)
    expect(peaks[BARS - 1]).toBeCloseTo(1)
  })

  it('keeps silence flat rather than dividing by zero', () => {
    expect(peaksFrom(new Float32Array(BARS * 4))).toEqual(
      Array.from({ length: BARS }, () => 0),
    )
  })

  it('has nothing to draw for an empty recording', () => {
    expect(peaksFrom(new Float32Array(0))).toEqual([])
  })
})

function mount(fallbackMs = 90_000) {
  render(
    <WaveScrubber
      blob={new Blob(['wav'])}
      label="The recording of the call"
      fallbackMs={fallbackMs}
    />,
  )
  return screen.getByRole('slider', { name: 'The recording of the call' })
}

describe('WaveScrubber', () => {
  it('shows the elapsed and the total time', () => {
    const slider = mount()
    expect(slider.getAttribute('aria-valuetext')).toBe('0:00 / 1:30')
    expect(screen.getByTestId('call-scrubber').textContent).toContain(
      '0:00 / 1:30',
    )
  })

  it('seeks with the arrow keys and stops at both ends', () => {
    const slider = mount(20_000)
    slider.focus()
    fireEvent.keyDown(slider, { key: 'ArrowUp' })
    expect(slider.getAttribute('aria-valuenow')).toBe('5')
    fireEvent.keyDown(slider, { key: 'ArrowDown' })
    fireEvent.keyDown(slider, { key: 'ArrowDown' })
    expect(slider.getAttribute('aria-valuenow')).toBe('0')
    fireEvent.keyDown(slider, { key: 'End' })
    expect(slider.getAttribute('aria-valuenow')).toBe('20')
    fireEvent.keyDown(slider, { key: 'ArrowRight' })
    expect(slider.getAttribute('aria-valuenow')).toBe('20')
  })

  it('draws a flat bar when nothing decoded the audio', () => {
    mount()
    expect(document.querySelector('.call-scrubber-flat')).not.toBeNull()
    expect(document.querySelector('.call-scrubber-bar')).toBeNull()
  })

  it('offers a play control', () => {
    mount()
    expect(
      screen.getByRole('button', { name: 'Play the recording' }),
    ).not.toBeNull()
  })
})
