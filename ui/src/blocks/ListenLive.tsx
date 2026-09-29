// Listen-Live in the Thread (ADR-0020): the minimal control on a
// `call` block. One button opens the listen socket and plays the mono
// mix with Web Audio; a second press closes it. Nothing is signalled
// to the Remote Party, and a listener who joins hears the call from
// that moment.

import { useCallback, useEffect, useRef, useState } from 'react'

import { Button } from '../primitives'
import type { Speaker } from '../ws/listen'
import { Listen, webAudioSpeaker } from '../ws/listen'

export type ListenState = 'idle' | 'listening' | 'ended' | 'failed'

export interface ListenLiveDeps {
  /** Injection point for tests; defaults to the native WebSocket. */
  createWebSocket?: (url: string) => WebSocket
  /** Injection point for tests; defaults to Web Audio. */
  createSpeaker?: () => Speaker
}

export interface ListenLiveHook {
  state: ListenState
  /** Why listening stopped, when it stopped badly. */
  message: string | null
  start: () => void
  stop: () => void
}

/** The listen socket of one call, as a React hook. */
export function useListenLive(
  callId: string,
  deps: ListenLiveDeps = {},
): ListenLiveHook {
  const [state, setState] = useState<ListenState>('idle')
  const [message, setMessage] = useState<string | null>(null)
  const listen = useRef<Listen | null>(null)
  const speaker = useRef<Speaker | null>(null)

  const close = useCallback(() => {
    listen.current?.stop()
    listen.current = null
    speaker.current?.close()
    speaker.current = null
  }, [])

  const stop = useCallback(() => {
    close()
    setState('idle')
    setMessage(null)
  }, [close])

  const start = useCallback(() => {
    if (listen.current !== null) return
    setMessage(null)
    const create = deps.createSpeaker ?? webAudioSpeaker
    speaker.current = create()
    listen.current = new Listen({
      url: `${socketOrigin()}/api/v1/calls/${callId}/listen`,
      createWebSocket: deps.createWebSocket,
      handlers: {
        onReady: () => setState('listening'),
        onFrame: (samples) => speaker.current?.play(samples),
        onEnded: () => {
          close()
          setState('ended')
        },
        onError: (failure) => {
          close()
          setState('failed')
          setMessage(failure)
        },
      },
    })
    listen.current.start()
  }, [callId, close, deps.createSpeaker, deps.createWebSocket])

  // A Thread that goes away takes the socket and the speakers with it.
  useEffect(() => close, [close])

  return { state, message, start, stop }
}

function socketOrigin(): string {
  const { protocol, host } = window.location
  return `${protocol === 'https:' ? 'wss:' : 'ws:'}//${host}`
}

/** The listen control of a `call` block. */
export function ListenLive({
  callId,
  deps = {},
}: {
  callId: string
  deps?: ListenLiveDeps
}) {
  const { state, message, start, stop } = useListenLive(callId, deps)
  const listening = state === 'listening'
  return (
    <div className="block-call" data-testid="call-block">
      <Button
        className="block-call-listen"
        onClick={listening ? stop : start}
      >
        {listening ? 'Stop listening' : 'Listen'}
      </Button>
      {state === 'ended' && <span className="block-call-note">Call ended</span>}
      {message !== null && <span className="block-call-note">{message}</span>}
    </div>
  )
}
