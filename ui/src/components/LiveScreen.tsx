// The live screen view: one WebRTC session per open tile. Media
// crosses the Media Relay: the container publishes no port, so
// the session's candidate is the relay's address and the ICE servers
// come from the daemon before the offer is made — a peer connection
// takes them when it is created and not later. Unmounting closes the
// peer connection, which tears the container-side session down. During
// a takeover the viewer's mouse and keyboard flow to the pipeline
// over the session's data channel; the pipeline drops them unless the
// user holds the switch.

import {
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from 'react'

import type { ApiClient } from '../api/client'
import { Button } from '../primitives'

import './LiveScreen.css'

/** The capture surface, fixed by the pipeline. */
const CAPTURE_WIDTH = 1280
const CAPTURE_HEIGHT = 800

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

  const send = useCallback((op: Record<string, unknown>) => {
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
          ;(event.target as HTMLElement).focus()
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
  return (
    <video
      ref={videoRef}
      className={interactive ? 'computer-live computer-live-driving' : 'computer-live'}
      autoPlay
      muted
      playsInline
      aria-label={`${agentName}'s live screen`}
      {...inputHandlers}
    />
  )
}
