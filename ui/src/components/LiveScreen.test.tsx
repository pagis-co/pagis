// The live screen view: the offer/answer handshake through the
// daemon, teardown on unmount, and the fallback when the session is
// refused.

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { LiveScreen } from './LiveScreen'

class FakeDataChannel {
  readyState = 'open'
  sent: string[] = []
  send = vi.fn((payload: string) => {
    this.sent.push(payload)
  })
}

class FakePeerConnection {
  static instances: FakePeerConnection[] = []
  configuration: RTCConfiguration | undefined
  ontrack: ((event: unknown) => void) | null = null
  onconnectionstatechange: (() => void) | null = null
  connectionState: RTCPeerConnectionState = 'new'
  closed = false
  localDescription: unknown = null
  remoteDescription: unknown = null
  channel: FakeDataChannel | null = null

  constructor(configuration?: RTCConfiguration) {
    this.configuration = configuration
    FakePeerConnection.instances.push(this)
  }

  createDataChannel = vi.fn((label: string) => {
    void label
    this.channel = new FakeDataChannel()
    return this.channel
  })
  addTransceiver = vi.fn()
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'v=0 offer' }))
  setLocalDescription = vi.fn(async (description: unknown) => {
    this.localDescription = description
  })
  setRemoteDescription = vi.fn(async (description: unknown) => {
    this.remoteDescription = description
  })
  close = vi.fn(() => {
    this.closed = true
  })
}

/** The relay answers no ICE server unless a test says otherwise. */
function stubApi(
  offer: () => Promise<{ data?: unknown; error?: unknown }>,
  ice: () => Promise<{ data?: unknown; error?: unknown }> = async () => ({
    data: { ice_servers: [] },
  }),
) {
  return { POST: vi.fn(offer), GET: vi.fn(ice) } as unknown as ApiClient
}

beforeEach(() => {
  FakePeerConnection.instances = []
  vi.stubGlobal('RTCPeerConnection', FakePeerConnection)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('LiveScreen', () => {
  it('reports a failed media connection and lets the user retry', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen api={api} agentId="ag1" agentName="Sage"
        interactive={false} fallback={<span>Last screen</span>} />,
    )
    await waitFor(() => expect(FakePeerConnection.instances).toHaveLength(1))
    const pc = FakePeerConnection.instances[0]
    await waitFor(() => expect(pc.remoteDescription).not.toBeNull())

    act(() => {
      pc.connectionState = 'failed'
      pc.onconnectionstatechange?.()
    })

    expect(await screen.findByRole('status')).toHaveProperty(
      'textContent', 'Live screen unavailable.',
    )
    expect(screen.getByText('Last screen')).toBeTruthy()
    expect(screen.queryByLabelText("Sage's live screen")).toBeNull()
    expect(pc.closed).toBe(true)

    fireEvent.click(screen.getByRole('button', { name: 'Retry live screen' }))
    await waitFor(() => expect(FakePeerConnection.instances).toHaveLength(2))
    expect(screen.queryByText('Last screen')).toBeNull()
    expect(screen.getByLabelText("Sage's live screen")).toBeTruthy()
    await waitFor(() => expect(FakePeerConnection.instances[1].remoteDescription).not.toBeNull())
  })

  it('offers through the daemon and applies the answer', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/screen/offer',
        {
          params: { path: { agent_id: 'ag1' } },
          body: { sdp: 'v=0 offer' },
        },
      ),
    )
    const pc = FakePeerConnection.instances[0]!
    expect(pc.addTransceiver).toHaveBeenCalledWith('video', {
      direction: 'recvonly',
    })
    await waitFor(() =>
      expect(pc.remoteDescription).toEqual({
        type: 'answer',
        sdp: 'v=0 answer',
      }),
    )
    expect(screen.getByLabelText("Sage's live screen")).toBeTruthy()
  })

  it("configures the relay's ICE servers before it offers", async () => {
    // The `turn` relay mints a credential per session, and a
    // peer connection takes its ICE servers only when it is made.
    const api = stubApi(
      async () => ({ data: { sdp: 'v=0 answer' } }),
      async () => ({
        data: {
          ice_servers: [
            {
              urls: ['turn:relay.example.net:3478'],
              username: '1800:abc',
              credential: 'signed',
            },
          ],
        },
      }),
    )
    render(
      <LiveScreen api={api} agentId="ag1" agentName="Sage"
        interactive={false} fallback={<span>fallback</span>} />,
    )

    await waitFor(() => expect(FakePeerConnection.instances.length).toBe(1))
    expect(api.GET).toHaveBeenCalledWith('/api/v1/screen/ice', {})
    expect(FakePeerConnection.instances[0].configuration).toEqual({
      iceServers: [
        {
          urls: ['turn:relay.example.net:3478'],
          username: '1800:abc',
          credential: 'signed',
        },
      ],
    })
  })

  it('a relay that answers nothing falls back', async () => {
    const api = stubApi(
      async () => ({ data: { sdp: 'v=0 answer' } }),
      async () => ({ error: { error: { message: 'no relay' } } }),
    )
    render(
      <LiveScreen api={api} agentId="ag1" agentName="Sage"
        interactive={false} fallback={<span>Last screen</span>} />,
    )

    expect(await screen.findByRole('status')).toHaveProperty(
      'textContent', 'Live screen unavailable.',
    )
    expect(FakePeerConnection.instances).toHaveLength(0)
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('closing the tile tears the session down', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    const view = render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )
    await waitFor(() => expect(FakePeerConnection.instances.length).toBe(1))

    view.unmount()

    expect(FakePeerConnection.instances[0].closed).toBe(true)
  })

  it('every session opens the input data channel', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )

    await waitFor(() => expect(FakePeerConnection.instances.length).toBe(1))
    expect(
      FakePeerConnection.instances[0].createDataChannel,
    ).toHaveBeenCalledWith('input')
  })

  it('while interactive, mouse and keyboard flow over the channel', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={true}
        fallback={<span>fallback</span>}
      />,
    )
    const video = await screen.findByLabelText("Sage's live screen")
    video.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 1280, height: 800 }) as DOMRect

    fireEvent.pointerDown(video, { clientX: 640, clientY: 400, button: 0 })
    fireEvent.pointerUp(video, { clientX: 640, clientY: 400, button: 0 })
    fireEvent.keyDown(video, { code: 'KeyA' })
    fireEvent.keyUp(video, { code: 'KeyA' })

    const channel = FakePeerConnection.instances[0].channel!
    const ops = channel.sent.map((payload) => JSON.parse(payload))
    expect(ops).toContainEqual({ op: 'move', x: 640, y: 400 })
    expect(ops).toContainEqual({ op: 'button', button: 'left', down: true })
    expect(ops).toContainEqual({ op: 'button', button: 'left', down: false })
    expect(ops).toContainEqual({ op: 'key', code: 'KeyA', down: true })
    expect(ops).toContainEqual({ op: 'key', code: 'KeyA', down: false })
  })

  it('while watching, input is not forwarded', async () => {
    const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )
    const video = await screen.findByLabelText("Sage's live screen")

    fireEvent.pointerDown(video, { clientX: 10, clientY: 10, button: 0 })
    fireEvent.keyDown(video, { code: 'KeyA' })

    expect(FakePeerConnection.instances[0].channel!.sent).toEqual([])
  })

  describe("a phone's on-screen keyboard", () => {
    /** A touch screen: the pointer is coarse. */
    function coarsePointer() {
      vi.stubGlobal('matchMedia', (query: string) => ({
        matches: query === '(pointer: coarse)',
        media: query,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
        dispatchEvent: () => false,
      }))
    }

    function renderScreen(interactive: boolean) {
      const api = stubApi(async () => ({ data: { sdp: 'v=0 answer' } }))
      render(
        <LiveScreen api={api} agentId="ag1" agentName="Sage"
          interactive={interactive} fallback={<span>fallback</span>} />,
      )
    }

    /** A Takeover on a touch screen with the keyboard open: the
     * textarea and the ops that left over the data channel. */
    async function openKeyboard() {
      coarsePointer()
      renderScreen(true)
      await waitFor(() => expect(FakePeerConnection.instances[0]?.channel).toBeTruthy())
      fireEvent.click(screen.getByRole('button', { name: 'Keyboard' }))
      const textarea = screen.getByLabelText("Text for Sage's screen") as HTMLTextAreaElement
      expect(document.activeElement).toBe(textarea)
      const channel = FakePeerConnection.instances[0].channel!
      return {
        textarea,
        sentinel: textarea.value,
        ops: () => channel.sent.map((payload) => JSON.parse(payload)),
      }
    }

    function beforeInput(
      target: HTMLElement,
      inputType: string,
      data: string | null = null,
      isComposing = false,
    ) {
      fireEvent(target, new InputEvent('beforeinput', {
        inputType, data, isComposing, bubbles: true, cancelable: true,
      }))
    }

    it('shows the Keyboard button on a touch screen during a Takeover only', async () => {
      coarsePointer()
      renderScreen(false)
      await screen.findByLabelText("Sage's live screen")
      expect(screen.queryByRole('button', { name: 'Keyboard' })).toBeNull()
      cleanup()

      coarsePointer()
      renderScreen(true)
      expect(await screen.findByRole('button', { name: 'Keyboard' })).toBeTruthy()
      cleanup()

      vi.unstubAllGlobals()
      vi.stubGlobal('RTCPeerConnection', FakePeerConnection)
      renderScreen(true)
      await screen.findByLabelText("Sage's live screen")
      expect(screen.queryByRole('button', { name: 'Keyboard' })).toBeNull()
    })

    it('gives the textarea the attributes that keep the phone from changing the text or the page', async () => {
      const { textarea } = await openKeyboard()
      expect(textarea.getAttribute('autocapitalize')).toBe('off')
      expect(textarea.getAttribute('autocomplete')).toBe('off')
      expect(textarea.getAttribute('autocorrect')).toBe('off')
      expect(textarea.getAttribute('spellcheck')).toBe('false')
      expect(textarea.getAttribute('enterkeyhint')).toBe('enter')
    })

    it('sends inserted text as one text op', async () => {
      const { textarea, ops } = await openKeyboard()

      beforeInput(textarea, 'insertText', 'é')

      expect(ops()).toEqual([{ op: 'text', text: 'é' }])
    })

    it('sends a composition as one text op at its end', async () => {
      const { textarea, ops } = await openKeyboard()

      fireEvent.compositionStart(textarea, { data: '' })
      beforeInput(textarea, 'insertCompositionText', 'n', true)
      fireEvent.compositionUpdate(textarea, { data: 'n' })
      beforeInput(textarea, 'insertCompositionText', 'ni', true)
      fireEvent.compositionUpdate(textarea, { data: 'ni' })
      beforeInput(textarea, 'deleteContentBackward', null, true)
      expect(ops()).toEqual([])

      fireEvent.compositionEnd(textarea, { data: '你' })

      expect(ops()).toEqual([{ op: 'text', text: '你' }])
    })

    it('sends Backspace for a delete and Enter for a new line', async () => {
      const { textarea, ops } = await openKeyboard()

      beforeInput(textarea, 'deleteContentBackward')
      beforeInput(textarea, 'insertLineBreak')
      beforeInput(textarea, 'insertParagraph')

      expect(ops()).toEqual([
        { op: 'key', code: 'Backspace', down: true },
        { op: 'key', code: 'Backspace', down: false },
        { op: 'key', code: 'Enter', down: true },
        { op: 'key', code: 'Enter', down: false },
        { op: 'key', code: 'Enter', down: true },
        { op: 'key', code: 'Enter', down: false },
      ])
    })

    it('splits a paste of 2000 characters into two text ops', async () => {
      const { textarea, ops } = await openKeyboard()
      // A character outside the Basic Multilingual Plane is two UTF-16
      // units in JavaScript and one character in screend.
      const pasted = '😀'.repeat(1500) + 'a'.repeat(500)

      beforeInput(textarea, 'insertFromPaste', pasted)

      expect(ops()).toEqual([
        { op: 'text', text: '😀'.repeat(1024) },
        { op: 'text', text: '😀'.repeat(476) + 'a'.repeat(500) },
      ])
    })

    it('sets the textarea back to the sentinel after each op', async () => {
      const { textarea, sentinel } = await openKeyboard()
      // An Android keyboard sends no delete for an empty field.
      expect(sentinel.length).toBeGreaterThan(0)
      const each = [
        () => beforeInput(textarea, 'insertText', 'a'),
        () => beforeInput(textarea, 'insertFromPaste', 'pasted'),
        () => beforeInput(textarea, 'deleteContentBackward'),
        () => beforeInput(textarea, 'insertLineBreak'),
        () => fireEvent.compositionEnd(textarea, { data: 'word' }),
      ]
      for (const op of each) {
        textarea.value = `${sentinel}left over`
        op()
        expect(textarea.value).toBe(sentinel)
        expect(textarea.selectionStart).toBe(sentinel.length)
      }
    })

    it('sets the textarea back to the sentinel after an edit that the page could not cancel', async () => {
      const { textarea, sentinel, ops } = await openKeyboard()

      textarea.value = sentinel.slice(1)
      fireEvent.input(textarea)

      expect(textarea.value).toBe(sentinel)
      expect(ops()).toEqual([])
    })

    it('keeps the focus in the textarea when the screen is tapped, so the keyboard stays open', async () => {
      const { textarea, ops } = await openKeyboard()
      const video = screen.getByLabelText("Sage's live screen")
      video.getBoundingClientRect = () =>
        ({ left: 0, top: 0, width: 1280, height: 800 }) as DOMRect

      fireEvent.pointerDown(video, { clientX: 640, clientY: 400, button: 0 })

      expect(document.activeElement).toBe(textarea)
      expect(ops()).toContainEqual({ op: 'button', button: 'left', down: true })
    })

    it('keeps the key path for a hardware keyboard', async () => {
      const { ops } = await openKeyboard()
      const video = screen.getByLabelText("Sage's live screen")

      fireEvent.keyDown(video, { code: 'KeyA' })
      fireEvent.keyUp(video, { code: 'KeyA' })

      expect(ops()).toEqual([
        { op: 'key', code: 'KeyA', down: true },
        { op: 'key', code: 'KeyA', down: false },
      ])
    })
  })

  it('a refused offer falls back', async () => {
    const api = stubApi(async () => ({
      error: { error: { code: 'computer_asleep', message: 'asleep' } },
    }))
    render(
      <LiveScreen
        api={api}
        agentId="ag1"
        agentName="Sage"
        interactive={false}
        fallback={<span>fallback</span>}
      />,
    )

    expect(await screen.findByText('fallback')).toBeTruthy()
  })
})
