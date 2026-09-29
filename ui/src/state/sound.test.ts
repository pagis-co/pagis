// The sound setting and the cues. Off is the default, the
// choice persists, and nothing sounds while the setting is off.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CUES, CuePlayer, type CueName } from '../sound/cues'
import { SOUND_KEY, cueForFrame, playCue, storedSound, useSound } from './sound'

class FakeParam {
  setValueAtTime = vi.fn()
  linearRampToValueAtTime = vi.fn()
  value = 0
}

class FakeOscillator {
  type = 'sine'
  frequency = new FakeParam()
  connect = vi.fn()
  start = vi.fn()
  stop = vi.fn()
}

class FakeAudioContext {
  static made = 0
  static oscillators: FakeOscillator[] = []
  currentTime = 0
  destination = {}
  resume = vi.fn()

  constructor() {
    FakeAudioContext.made += 1
  }

  createOscillator = () => {
    const oscillator = new FakeOscillator()
    FakeAudioContext.oscillators.push(oscillator)
    return oscillator as unknown as OscillatorNode
  }
  createGain = () =>
    ({ gain: new FakeParam(), connect: vi.fn() }) as unknown as GainNode
}

beforeEach(() => {
  FakeAudioContext.made = 0
  FakeAudioContext.oscillators = []
  window.localStorage.clear()
  vi.stubGlobal('AudioContext', FakeAudioContext)
  useSound.setState({ enabled: false })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the sound setting', () => {
  it('is off until the user turns it on', () => {
    expect(storedSound()).toBe(false)
  })

  it('keeps the choice in this browser', () => {
    useSound.getState().setEnabled(true)

    expect(window.localStorage.getItem(SOUND_KEY)).toBe('on')
    expect(storedSound()).toBe(true)

    useSound.getState().setEnabled(false)
    expect(storedSound()).toBe(false)
  })

  it('plays nothing while it is off', () => {
    for (const name of Object.keys(CUES) as CueName[]) playCue(name)

    expect(FakeAudioContext.made).toBe(0)
    expect(FakeAudioContext.oscillators).toEqual([])
  })

  it('plays the cue once it is on', () => {
    useSound.getState().setEnabled(true)

    playCue('approval')

    expect(FakeAudioContext.oscillators.length).toBe(CUES.approval.length)
    for (const oscillator of FakeAudioContext.oscillators) {
      expect(oscillator.start).toHaveBeenCalled()
      expect(oscillator.stop).toHaveBeenCalled()
    }
  })
})

describe('the cue a frame earns', () => {
  it('names the four moments', () => {
    expect(cueForFrame('request.created', { kind: 'tool_action' })).toBe(
      'approval',
    )
    expect(cueForFrame('request.created', { kind: 'credential_action' })).toBe(
      'approval',
    )
    expect(cueForFrame('call.placed', {})).toBe('ringing')
    expect(cueForFrame('call.answered', {})).toBe('connected')
    expect(cueForFrame('run.state_changed', { to: 'failed' })).toBe('failed')
  })

  it('stays silent on every other frame', () => {
    // A form or a choice is a question, not an approval.
    expect(cueForFrame('request.created', { kind: 'form' })).toBeNull()
    expect(cueForFrame('request.created', { kind: 'choice' })).toBeNull()
    expect(cueForFrame('run.state_changed', { to: 'completed' })).toBeNull()
    expect(cueForFrame('call.ended', {})).toBeNull()
    expect(cueForFrame('message.completed', {})).toBeNull()
  })
})

describe('the cue player', () => {
  it('gives every cue its own shape', () => {
    const names = Object.keys(CUES) as CueName[]
    expect(names).toEqual(['approval', 'ringing', 'connected', 'failed'])
    const shapes = names.map((name) => JSON.stringify(CUES[name]))
    expect(new Set(shapes).size).toBe(names.length)
  })

  it('makes one audio context for every cue it plays', () => {
    const player = new CuePlayer()

    player.play('ringing')
    player.play('failed')

    expect(FakeAudioContext.made).toBe(1)
  })

  it('stays silent where the browser has no Web Audio API', () => {
    vi.stubGlobal('AudioContext', undefined)

    expect(() => new CuePlayer().play('failed')).not.toThrow()
  })
})
