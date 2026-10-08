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
// The expanded view zooms and pans the video with two fingers, and a
// point maps to the capture pixel under it through the bars of
// `object-fit: contain` and through the zoom (screenView, screenGestures).

import {
  type ReactNode,
  type RefObject,
  useCallback,
  useEffect,
  useRef,
  useState,
} from 'react'

import type { ApiClient } from '../api/client'
import { Keyboard } from 'lucide-react'
import { Button, Textarea } from '../primitives'
import { useMediaQuery } from '../state/useIsMobile'
import {
  type GestureEvent,
  type GestureOutput,
  initialGestureState,
  stepGestures,
} from './screenGestures'
import {
  IDENTITY_VIEW,
  type Rect,
  type View,
  applyViewChange,
  capturePerPixel,
  capturePoint,
} from './screenView'

import './LiveScreen.css'
import { createPortal } from 'react-dom'

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

/** The live screen in the tile, in the expanded view, or in the expanded
 * view while the Person holds the switch. */
export type ScreenMode = 'compact' | 'expanded' | 'takeover'

/** The pixels of a wheel delta that make one wheel click of a `scroll`
 * op. */
const PIXELS_PER_CLICK = 120

/** The data channel op of a gesture's input output. */
function inputOp(
  output: Exclude<GestureOutput, { kind: 'view' }>,
  box: Rect,
  view: View,
): Op {
  switch (output.kind) {
    case 'move':
      return { op: 'move', ...capturePoint(output.point, box, view) }
    case 'button':
      return { op: 'button', button: output.button, down: output.down }
    case 'scroll': {
      // The page moves as far as the finger, in capture pixels.
      const clicks = capturePerPixel(box, view) / PIXELS_PER_CLICK
      return { op: 'scroll', dx: output.dx * clicks, dy: output.dy * clicks }
    }
  }
}

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
  inFooter = false,
}: {
  agentName: string
  inputRef: RefObject<HTMLTextAreaElement | null>
  send: (op: Op) => void
  inFooter?: boolean
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
      <Button size={inFooter ? 'lg' : 'sm'} onClick={open}>{inFooter && <Keyboard size={20} aria-hidden />}Keyboard</Button>
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
  mode,
  fallback,
  keyboardTarget,
}: {
  api: ApiClient
  agentId: string
  agentName: string
  /** In a Takeover the input goes to the Computer. The expanded view
   * zooms and pans. */
  mode: ScreenMode
  /** Shown while the session is down: connect failed or was refused. */
  fallback: ReactNode
  /** The phone places the real keyboard control beside Hand back. */
  keyboardTarget?: HTMLElement | null
}) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const viewportRef = useRef<HTMLDivElement>(null)
  const channelRef = useRef<RTCDataChannel | null>(null)
  const keyboardRef = useRef<HTMLTextAreaElement>(null)
  const coarsePointer = useMediaQuery(COARSE_POINTER_QUERY)
  const [failed, setFailed] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const interactive = mode === 'takeover'
  // The gesture state and the view change on each pointer event. The
  // refs hold the current values for the next event, and the state
  // draws the view.
  const gesturesRef = useRef(initialGestureState)
  const viewRef = useRef<View>(IDENTITY_VIEW)
  const [view, setView] = useState<View>(IDENTITY_VIEW)

  // The tile shows the whole screen, and the next expanded view starts
  // with no zoom.
  useEffect(() => {
    if (mode !== 'compact') return
    gesturesRef.current = initialGestureState
    viewRef.current = IDENTITY_VIEW
    setView(IDENTITY_VIEW)
  }, [mode])

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

  /** One pointer event through the gestures. The view changes in every
   * expanded view, and the input goes out only in a Takeover. A point
   * maps through the box of the viewport, which the transform of the
   * video does not change. */
  const gesture = (type: GestureEvent['type']) => (event: React.PointerEvent) => {
    const viewport = viewportRef.current
    if (viewport === null) return
    const [next, outputs] = stepGestures(gesturesRef.current, {
      type,
      id: event.pointerId,
      pointerType: event.pointerType,
      point: { x: event.clientX, y: event.clientY },
      time: event.timeStamp,
      button: event.button,
    })
    gesturesRef.current = next
    const box = viewport.getBoundingClientRect()
    let current = viewRef.current
    for (const output of outputs) {
      if (output.kind === 'view') {
        current = applyViewChange(current, output.change, box)
      } else if (interactive) {
        send(inputOp(output, box, current))
      }
    }
    if (current !== viewRef.current) {
      viewRef.current = current
      setView(current)
    }
  }

  const gestureHandlers = mode === 'compact'
    ? {}
    : {
        onPointerDown: (event: React.PointerEvent) => {
          if (interactive) {
            // While the on-screen keyboard is open, a tap keeps the
            // focus in its textarea, so the keyboard stays open.
            const keyboard = keyboardRef.current
            if (keyboard !== null && document.activeElement === keyboard) {
              event.preventDefault()
            } else {
              ;(event.target as HTMLElement).focus()
            }
          }
          gesture('down')(event)
        },
        onPointerMove: gesture('move'),
        onPointerUp: gesture('up'),
        onPointerCancel: gesture('cancel'),
        // A hold is a right click, so the browser shows no menu.
        onContextMenu: (event: React.MouseEvent) => event.preventDefault(),
      }

  const inputHandlers = interactive
    ? {
        tabIndex: 0,
        onWheel: (event: React.WheelEvent) =>
          send({
            op: 'scroll',
            dx: event.deltaX / PIXELS_PER_CLICK,
            dy: event.deltaY / PIXELS_PER_CLICK,
          }),
        onKeyDown: (event: React.KeyboardEvent) => {
          event.preventDefault()
          send({ op: 'key', code: event.code, down: true })
        },
        onKeyUp: (event: React.KeyboardEvent) => {
          event.preventDefault()
          send({ op: 'key', code: event.code, down: false })
        },
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
  const className = [
    'computer-live',
    mode !== 'compact' && 'computer-live-zoomable',
    interactive && 'computer-live-driving',
  ].filter(Boolean).join(' ')
  // The video keeps its place in the tree when the keyboard comes and
  // goes, so its stream survives the change.
  return (
    <>
      <div ref={viewportRef} className="computer-live-viewport">
        <video
          ref={videoRef}
          className={className}
          style={mode === 'compact' ? undefined : {
            transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
          }}
          autoPlay
          muted
          playsInline
          aria-label={`${agentName}'s live screen`}
          {...gestureHandlers}
          {...inputHandlers}
        />
      </div>
      {interactive && coarsePointer && (keyboardTarget
        ? createPortal(<ScreenKeyboard agentName={agentName} inputRef={keyboardRef} send={send} inFooter />, keyboardTarget)
        : <ScreenKeyboard agentName={agentName} inputRef={keyboardRef} send={send} />)}
    </>
  )
}
