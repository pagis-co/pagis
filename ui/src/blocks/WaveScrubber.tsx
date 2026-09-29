// The recording player of a call card. The native `<audio>`
// control draws differently on every operating system and reads as a
// foreign object in the card, so the card draws its own: one play
// control, a waveform, and the elapsed and total time.
//
// The waveform comes from the audio itself when the browser can decode
// it, and is a flat bar when it cannot (an unsupported codec, or a test
// environment with no Web Audio). The scrubber is a slider: it takes
// focus, the arrow keys seek, Home and End go to the ends, and the
// space bar plays and pauses.

import { useCallback, useEffect, useRef, useState } from 'react'
import { Pause, Play } from 'lucide-react'

import { IconButton } from '../primitives'
import { formatDuration } from './call'

/** The bars a waveform is drawn with. */
export const BARS = 72

/** How far one arrow press seeks, in seconds. */
const STEP = 5

/** The loudness of each bar: the peak of the samples it covers, scaled
 *  so the loudest bar is a full one. */
export function peaksFrom(
  samples: Float32Array,
  bars: number = BARS,
): number[] {
  if (samples.length === 0) return []
  const span = Math.max(1, Math.floor(samples.length / bars))
  const peaks: number[] = []
  for (let bar = 0; bar < bars; bar += 1) {
    let peak = 0
    const from = bar * span
    for (let index = from; index < from + span && index < samples.length; index += 1) {
      const level = Math.abs(samples[index])
      if (level > peak) peak = level
    }
    peaks.push(peak)
  }
  const loudest = Math.max(...peaks)
  return loudest === 0 ? peaks : peaks.map((peak) => peak / loudest)
}

/** The decoded peaks of one recording, or null while they are unknown.
 *  A browser that cannot decode the blob keeps the flat bar. */
export function useWaveform(blob: Blob | null): number[] | null {
  const [peaks, setPeaks] = useState<number[] | null>(null)
  useEffect(() => {
    setPeaks(null)
    if (blob === null) return
    const Context =
      window.AudioContext ??
      (window as unknown as { webkitAudioContext?: typeof AudioContext })
        .webkitAudioContext
    if (Context === undefined) return
    let canceled = false
    const context = new Context()
    blob
      .arrayBuffer()
      .then((bytes) => context.decodeAudioData(bytes))
      .then((decoded) => {
        if (!canceled) setPeaks(peaksFrom(decoded.getChannelData(0)))
      })
      .catch(() => undefined)
      .finally(() => {
        void context.close().catch(() => undefined)
      })
    return () => {
      canceled = true
    }
  }, [blob])
  return peaks
}

export interface WaveScrubberProps {
  /** The recording. */
  blob: Blob
  /** What the slider is called, for a screen reader. */
  label: string
  /** The length the card knows from the Call record, in milliseconds.
   *  It stands in until the browser reads the real one. */
  fallbackMs: number
}

export function WaveScrubber({ blob, label, fallbackMs }: WaveScrubberProps) {
  const audio = useRef<HTMLAudioElement | null>(null)
  const [url, setUrl] = useState<string | null>(null)
  const [at, setAt] = useState(0)
  const [total, setTotal] = useState(fallbackMs / 1000)
  const [playing, setPlaying] = useState(false)
  const peaks = useWaveform(blob)

  useEffect(() => {
    const objectUrl = URL.createObjectURL(blob)
    setUrl(objectUrl)
    return () => URL.revokeObjectURL(objectUrl)
  }, [blob])

  const seek = useCallback(
    (to: number) => {
      const bounded = Math.min(Math.max(0, to), total)
      setAt(bounded)
      if (audio.current !== null) audio.current.currentTime = bounded
    },
    [total],
  )

  const toggle = useCallback(() => {
    const element = audio.current
    if (element === null) return
    if (playing) {
      element.pause()
      setPlaying(false)
      return
    }
    const started = element.play() as Promise<void> | undefined
    started?.catch(() => setPlaying(false))
    setPlaying(true)
  }, [playing])

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const keys: Record<string, () => void> = {
      ArrowRight: () => seek(at + STEP),
      ArrowUp: () => seek(at + STEP),
      ArrowLeft: () => seek(at - STEP),
      ArrowDown: () => seek(at - STEP),
      Home: () => seek(0),
      End: () => seek(total),
      ' ': toggle,
    }
    const act = keys[event.key]
    if (act === undefined) return
    event.preventDefault()
    act()
  }

  const done = total > 0 ? at / total : 0
  const bars = peaks ?? []
  const clock = `${formatDuration(at * 1000)} / ${formatDuration(total * 1000)}`

  return (
    <div className="call-scrubber" data-testid="call-scrubber">
      <IconButton
        icon={playing ? Pause : Play}
        label={playing ? 'Pause the recording' : 'Play the recording'}
        onClick={toggle}
      />
      <div
        className="call-scrubber-track"
        role="slider"
        tabIndex={0}
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={Math.round(total)}
        aria-valuenow={Math.round(at)}
        aria-valuetext={clock}
        onKeyDown={onKeyDown}
        onClick={(event) => {
          const box = event.currentTarget.getBoundingClientRect()
          if (box.width > 0) seek(((event.clientX - box.left) / box.width) * total)
        }}
      >
        {bars.length === 0 ? (
          <div className="call-scrubber-flat" aria-hidden />
        ) : (
          <div className="call-scrubber-bars" aria-hidden>
            {bars.map((peak, index) => (
              <span
                key={index}
                className={
                  index / bars.length <= done
                    ? 'call-scrubber-bar call-scrubber-bar-played'
                    : 'call-scrubber-bar'
                }
                style={{ height: `${Math.max(8, peak * 100)}%` }}
              />
            ))}
          </div>
        )}
        <span
          className="call-scrubber-head"
          style={{ left: `${done * 100}%` }}
          aria-hidden
        />
      </div>
      <span className="call-scrubber-clock">{clock}</span>
      {url !== null && (
        <audio
          ref={audio}
          src={url}
          preload="metadata"
          onTimeUpdate={(event) => setAt(event.currentTarget.currentTime)}
          onLoadedMetadata={(event) => {
            const length = event.currentTarget.duration
            if (Number.isFinite(length) && length > 0) setTotal(length)
          }}
          onEnded={() => setPlaying(false)}
        />
      )}
    </div>
  )
}
