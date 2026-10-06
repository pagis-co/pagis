// The live screen view: one WebRTC session per open tile. Media
// crosses the Media Relay: the container publishes no port, so
// the session's candidate is the relay's address and the ICE servers
// come from the daemon before the offer is made — a peer connection
// takes them when it is created and not later. Unmounting closes the
// peer connection, which tears the container-side session down. During
// a takeover the viewer's mouse and keyboard flow to the pipeline
// over the session's data channel; the pipeline drops them unless the
// user holds the switch. On a touch screen a Keyboard button opens the
// on-screen keyboard, whose text goes over the same channel as text.

import {
  type ReactNode,
  type RefObject,
  useCallback,
  useEffect,
  useRef,
  useState,
} from 'react'

import type { ApiClient } from '../api/client'
import { Button, Textarea } from '../primitives'
import { useMediaQuery } from '../state/useIsMobile'

import './LiveScreen.css'

/** The capture surface, fixed by the pipeline. */
const CAPTURE_WIDTH = 1280
const CAPTURE_HEIGHT = 800

/** A touch screen, where a phone or a tablet gives an on-screen keyboard. */
const COARSE_POINTER_QUERY = '(pointer: coarse)'

/** The most characters of one `text` op. screend refuses a longer text. */
const TEXT_LIMIT = 1024

/** The text that the keyboard textarea holds between ops. An Android
 * keyboard sends no delete event for an empty field, so the sentinel
 * gives each Backspace a character to remove. The characters are zero
 * width spaces, as in Apache Guacamole, so no keyboard reads them as a
 * word to correct. */
const SENTINEL = '\u200b'.repeat(4)

type Op = Record<string, unknown>

/** The `text` ops of a text, each of at most `TEXT_LIMIT` characters.
 * screend counts code points, so a split never cuts a surrogate pair. */
function textOps(text: string): Op[] {
  const characters = Array.from(text)
  const ops: Op[] = []
  for (let start = 0; start < characters.length; start += TEXT_LIMIT) {
    ops.push({ op: 'text', text: characters.slice(start, start + TEXT_LIMIT).join('') })
  }
  return ops
}

/** The ICE servers of the installation's Media Relay. */
async function iceServers(api: ApiClient): Promise<RTCIceServer[]> {
  const { data, error } = await api.GET('/api/v1/screen/ice', {})
  if (data === undefined) {
    const detail = (error as { error?: { message?: string } } | undefined)
      ?.error
    throw new Error(detail?.message ?? 'screen relay unavailable')
  }
  return data.ice_servers.map((server) => ({
    urls: server.urls,
    username: server.username,
    credential: server.credential,
  }))
}

async function connect(
  api: ApiClient,
  agentId: string,
  pc: RTCPeerConnection,
): Promise<void> {
  pc.addTransceiver('video', { direction: 'recvonly' })
  const offer = await pc.createOffer()
  await pc.setLocalDescription(offer)
  const { data, error } = await api.POST(
    '/api/v1/agents/{agent_id}/screen/offer',
    {
      params: { path: { agent_id: agentId } },
      body: { sdp: offer.sdp ?? '' },
    },
  )
  if (data === undefined) {
    const detail = (error as { error?: { message?: string } } | undefined)
      ?.error
    throw new Error(detail?.message ?? 'screen offer failed')
  }
  await pc.setRemoteDescription({ type: 'answer', sdp: data.sdp })
}

/** The Keyboard button of a touch screen and the hidden textarea that
 * takes the on-screen keyboard's text. A phone gives no on-screen
 * keyboard for a focused video, and an on-screen keyboard gives no
 * reliable key code, so the textarea reads `beforeinput` and
 * composition events, as xterm.js and Apache Guacamole do, and sends
 * text as text. */
function ScreenKeyboard({
  agentName,
  inputRef,
  send,
}: {
  agentName: string
  inputRef: RefObject<HTMLTextAreaElement | null>
  send: (op: Op) => void
}) {
  useEffect(() => {
    const input = inputRef.current
    if (input === null) return
    const reset = () => {
      input.value = SENTINEL
      input.setSelectionRange(SENTINEL.length, SENTINEL.length)
    }
    const press = (code: string) => {
      send({ op: 'key', code, down: true })
      send({ op: 'key', code, down: false })
    }
    const sendText = (text: string) => textOps(text).forEach(send)
    const onBeforeInput = (event: InputEvent) => {
      // A composition sends its text once, at its end.
      if (event.isComposing) return
      switch (event.inputType) {
        case 'insertText':
        case 'insertFromPaste':
          sendText(event.data ?? event.dataTransfer?.getData('text/plain') ?? '')
          break
        case 'deleteContentBackward':
          press('Backspace')
          break
        case 'insertLineBreak':
        case 'insertParagraph':
          press('Enter')
          break
        default:
          return
      }
      event.preventDefault()
      reset()
    }
    const onCompositionEnd = (event: CompositionEvent) => {
      sendText(event.data)
      reset()
    }
    // A keyboard can change the field where the page cannot cancel the
    // change, so each edit outside a composition ends at the sentinel.
    const onInput = (event: Event) => {
      if (!(event as InputEvent).isComposing) reset()
    }
    reset()
    input.addEventListener('beforeinput', onBeforeInput)
    input.addEventListener('compositionend', onCompositionEnd)
    input.addEventListener('input', onInput)
    input.addEventListener('focus', reset)
    return () => {
      input.removeEventListener('beforeinput', onBeforeInput)
      input.removeEventListener('compositionend', onCompositionEnd)
      input.removeEventListener('input', onInput)
      input.removeEventListener('focus', reset)
    }
  }, [inputRef, send])

  // iOS and Android open the on-screen keyboard only for a focus that
  // a tap gives. A field that keeps the focus after the Person closed
  // the keyboard opens it again only when it takes the focus again.
  const open = () => {
    const input = inputRef.current
    if (input === null) return
    if (document.activeElement === input) input.blur()
    input.focus()
  }

  return (
    <div className="computer-live-keyboard">
      <Button size="sm" onClick={open}>Keyboard</Button>
      <Textarea
        ref={inputRef}
        bare
        className="computer-live-keyboard-input"
        aria-label={`Text for ${agentName}'s screen`}
        autoCapitalize="off"
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
        enterKeyHint="enter"
      />
    </div>
  )
}

export function LiveScreen({
  api,
  agentId,
  agentName,
  interactive,
  fallback,
}: {
  api: ApiClient
  agentId: string
  agentName: string
  /** The user holds the input switch: forward mouse + keyboard. */
  interactive: boolean
  /** Shown while the session is down: connect failed or was refused. */
  fallback: ReactNode
}) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const channelRef = useRef<RTCDataChannel | null>(null)
  const keyboardRef = useRef<HTMLTextAreaElement>(null)
  const coarsePointer = useMediaQuery(COARSE_POINTER_QUERY)
  const [failed, setFailed] = useState(false)
  const [attempt, setAttempt] = useState(0)

  useEffect(() => {
    setFailed(false)
    let disposed = false
    let pc: RTCPeerConnection | null = null
    const close = () => {
      disposed = true
      channelRef.current = null
      if (pc !== null) {
        pc.ontrack = null
        pc.onconnectionstatechange = null
        pc.close()
      }
    }
    const fail = () => {
      if (disposed) return
      close()
      setFailed(true)
    }
    const start = async () => {
      const servers = await iceServers(api)
      if (disposed) return
      const peer = new RTCPeerConnection({ iceServers: servers })
      pc = peer
      peer.onconnectionstatechange = () => {
        if (peer.connectionState === 'failed') fail()
      }
      // The input channel exists on every session; the pipeline
      // ignores it unless the user holds the switch.
      channelRef.current = peer.createDataChannel('input')
      peer.ontrack = (event) => {
        if (videoRef.current) {
          videoRef.current.srcObject =
            event.streams[0] ?? new MediaStream([event.track])
        }
      }
      await connect(api, agentId, peer)
    }
    start().catch(fail)
    return close
  }, [api, agentId, attempt])

  const send = useCallback((op: Op) => {
    const channel = channelRef.current
    if (channel !== null && channel.readyState === 'open') {
      channel.send(JSON.stringify(op))
    }
  }, [])

  /** Element coordinates -> capture pixels. */
  const capturePoint = useCallback((event: React.PointerEvent) => {
    const rect = (event.target as HTMLElement).getBoundingClientRect()
    return {
      x: ((event.clientX - rect.left) / rect.width) * CAPTURE_WIDTH,
      y: ((event.clientY - rect.top) / rect.height) * CAPTURE_HEIGHT,
    }
  }, [])

  const button = (event: React.PointerEvent) =>
    event.button === 2 ? 'right' : event.button === 1 ? 'middle' : 'left'

  const inputHandlers = interactive
    ? {
        tabIndex: 0,
        onPointerMove: (event: React.PointerEvent) =>
          send({ op: 'move', ...capturePoint(event) }),
        onPointerDown: (event: React.PointerEvent) => {
          // While the on-screen keyboard is open, a tap keeps the focus
          // in its textarea, so the keyboard stays open.
          const keyboard = keyboardRef.current
          if (keyboard !== null && document.activeElement === keyboard) {
            event.preventDefault()
          } else {
            ;(event.target as HTMLElement).focus()
          }
          send({ op: 'move', ...capturePoint(event) })
          send({ op: 'button', button: button(event), down: true })
        },
        onPointerUp: (event: React.PointerEvent) =>
          send({ op: 'button', button: button(event), down: false }),
        onWheel: (event: React.WheelEvent) =>
          send({ op: 'scroll', dx: event.deltaX / 120, dy: event.deltaY / 120 }),
        onKeyDown: (event: React.KeyboardEvent) => {
          event.preventDefault()
          send({ op: 'key', code: event.code, down: true })
        },
        onKeyUp: (event: React.KeyboardEvent) => {
          event.preventDefault()
          send({ op: 'key', code: event.code, down: false })
        },
        onContextMenu: (event: React.MouseEvent) => event.preventDefault(),
      }
    : {}

  if (failed) {
    return (
      <div className="computer-live-unavailable">
        <p role="status">Live screen unavailable.</p>
        {fallback}
        <Button onClick={() => {
          setFailed(false)
          setAttempt((value) => value + 1)
        }}>Retry live screen</Button>
      </div>
    )
  }
  // The video keeps its place in the tree when the keyboard comes and
  // goes, so its stream survives the change.
  return (
    <>
      <video
        ref={videoRef}
        className={interactive ? 'computer-live computer-live-driving' : 'computer-live'}
        autoPlay
        muted
        playsInline
        aria-label={`${agentName}'s live screen`}
        {...inputHandlers}
      />
      {interactive && coarsePointer && (
        <ScreenKeyboard agentName={agentName} inputRef={keyboardRef} send={send} />
      )}
    </>
  )
}
