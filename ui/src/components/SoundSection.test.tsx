// The Sound section: a switch in one frame, the four cues in a
// second frame, and a hint that says what the page cannot do.

import { fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { SOUND_KEY, useSound } from '../state/sound'
import { SoundSection } from './SoundSection'

class FakeAudioContext {
  static made = 0
  currentTime = 0
  destination = {}
  resume = vi.fn()
  constructor() {
    FakeAudioContext.made += 1
  }
  createOscillator = () =>
    ({
      frequency: { value: 0 },
      connect: vi.fn(),
      start: vi.fn(),
      stop: vi.fn(),
    }) as unknown as OscillatorNode
  createGain = () =>
    ({
      gain: { setValueAtTime: vi.fn(), linearRampToValueAtTime: vi.fn() },
      connect: vi.fn(),
    }) as unknown as GainNode
}

beforeEach(() => {
  FakeAudioContext.made = 0
  window.localStorage.clear()
  vi.stubGlobal('AudioContext', FakeAudioContext)
  useSound.setState({ enabled: false })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('SoundSection', () => {
  it('shows the title line and the closing hint', () => {
    render(<SoundSection />)
    expect(screen.getByRole('heading', { name: 'Sound' })).toBeTruthy()
    expect(screen.getByText('Four cues, all optional.')).toBeTruthy()
    expect(screen.getByText(/the switch does\.$/)).toBeTruthy()
  })

  it('turns sound on with a switch and keeps the choice', () => {
    render(<SoundSection />)
    const toggle = screen.getByRole('switch', { name: 'Play sound cues' })
    expect(toggle.getAttribute('aria-checked')).toBe('false')

    fireEvent.click(toggle)

    expect(toggle.getAttribute('aria-checked')).toBe('true')
    expect(useSound.getState().enabled).toBe(true)
    expect(window.localStorage.getItem(SOUND_KEY)).toBe('on')
  })

  it('lists the four cues with what each one means', () => {
    render(<SoundSection />)
    for (const [name, meaning] of [
      ['Approval needed', 'something waits for you'],
      ['Call ringing', 'an inbound call before a sprite answers'],
      ['Call connected', 'the call is live'],
      ['Run failed', 'a run ended failed'],
    ]) {
      expect(screen.getByText(name)).toBeTruthy()
      expect(screen.getByText(meaning)).toBeTruthy()
    }
    expect(screen.getAllByRole('button', { name: /^Hear the cue for:/ })).toHaveLength(4)
  })

  it('cannot be heard while sound is off', () => {
    render(<SoundSection />)
    const hear = screen.getByRole('button', {
      name: 'Hear the cue for: Run failed',
    }) as HTMLButtonElement
    expect(hear.disabled).toBe(true)

    fireEvent.click(hear)
    expect(FakeAudioContext.made).toBe(0)
  })

  it('plays a cue once sound is on', () => {
    render(<SoundSection />)
    fireEvent.click(screen.getByRole('switch', { name: 'Play sound cues' }))

    fireEvent.click(screen.getByRole('button', { name: 'Hear the cue for: Run failed' }))

    expect(FakeAudioContext.made).toBe(1)
  })
})
