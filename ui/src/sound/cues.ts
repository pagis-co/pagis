// The four sound cues (docs/UI-DESIGN.md):
// approval needed, call ringing, call connected, run failed. Each one
// is a pair of short tones the Web Audio API makes on the spot, so the
// product ships no audio file and no decoder.
//
// This module only makes sound. Whether a cue may sound at all is the
// setting's decision (`src/state/sound.ts`).

export type CueName = 'approval' | 'ringing' | 'connected' | 'failed'

/** One tone of a cue: when it starts, how long it lasts, its pitch. */
export interface Tone {
  /** Hertz. */
  hz: number
  /** Milliseconds after the cue starts. */
  atMs: number
  ms: number
  type: OscillatorType
}

/** The four cues. A rise asks, a fall reports, a repeat rings. */
export const CUES: Record<CueName, readonly Tone[]> = {
  approval: [
    { hz: 660, atMs: 0, ms: 90, type: 'triangle' },
    { hz: 880, atMs: 90, ms: 120, type: 'triangle' },
  ],
  ringing: [
    { hz: 480, atMs: 0, ms: 180, type: 'sine' },
    { hz: 480, atMs: 260, ms: 180, type: 'sine' },
  ],
  connected: [
    { hz: 520, atMs: 0, ms: 80, type: 'sine' },
    { hz: 784, atMs: 80, ms: 140, type: 'sine' },
  ],
  failed: [
    { hz: 400, atMs: 0, ms: 110, type: 'sawtooth' },
    { hz: 220, atMs: 110, ms: 200, type: 'sawtooth' },
  ],
}

/** Loud enough to hear over a room, quiet enough beside a voice call. */
const PEAK = 0.08
/** The ramp that keeps a tone from clicking at either end. */
const EDGE_S = 0.012

/**
 * The cue player. It holds one audio context, made on the first cue
 * and kept, because a context is expensive and a browser caps how many
 * a page may have.
 */
export class CuePlayer {
  private context: AudioContext | null = null

  /** Play one cue. A browser without the Web Audio API stays silent. */
  play(name: CueName): void {
    const context = this.open()
    if (context === null) return
    // A context made before the first gesture starts suspended; the
    // resume is a promise the cue does not wait for.
    void context.resume?.()
    const start = context.currentTime
    for (const tone of CUES[name]) {
      this.tone(context, tone, start + tone.atMs / 1000)
    }
  }

  private open(): AudioContext | null {
    if (this.context !== null) return this.context
    const Ctor = (
      globalThis as { AudioContext?: new () => AudioContext }
    ).AudioContext
    if (Ctor === undefined) return null
    this.context = new Ctor()
    return this.context
  }

  private tone(context: AudioContext, tone: Tone, at: number): void {
    const oscillator = context.createOscillator()
    const gain = context.createGain()
    oscillator.type = tone.type
    oscillator.frequency.value = tone.hz
    const end = at + tone.ms / 1000
    gain.gain.setValueAtTime(0, at)
    gain.gain.linearRampToValueAtTime(PEAK, at + EDGE_S)
    gain.gain.setValueAtTime(PEAK, end - EDGE_S)
    gain.gain.linearRampToValueAtTime(0, end)
    oscillator.connect(gain)
    gain.connect(context.destination)
    oscillator.start(at)
    oscillator.stop(end)
  }
}
