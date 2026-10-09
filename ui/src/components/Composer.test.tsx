// Composer attachments: a pasted or attached file uploads as an
// artifact right away, shows as a chip, and the send carries the
// artifact ids. Push-to-talk (ADR-0020): hold the microphone,
// the transcript lands as a draft, and the user presses send.

import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { uploadArtifact, type ApiClient, type ArtifactDto } from '../api/client'
import { useSendMessage } from '../queries'
import { useComposerDraft } from '../state/composerDraft'
import { useSpeaking } from '../state/stores'
import type { DictationHandlers, DictationOptions } from '../ws/dictation'
import { Composer } from './Composer'

vi.mock('../api/client', () => ({
  uploadArtifact: vi.fn(),
}))

vi.mock('../queries', () => ({
  useSendMessage: vi.fn(),
}))

const dictations: { options: DictationOptions; released: number; canceled: number }[] =
  []

vi.mock('../ws/dictation', () => ({
  Dictation: class {
    constructor(options: DictationOptions) {
      dictations.push({ options, released: 0, canceled: 0 })
    }
    start() {}
    release() {
      dictations[dictations.length - 1].released += 1
    }
    cancel() {
      dictations[dictations.length - 1].canceled += 1
    }
  },
}))

const mutate = vi.fn()

function dto(id: string, filename: string): ArtifactDto {
  return {
    id,
    filename,
    mime: 'image/png',
    size_bytes: 3,
    sha256: 'ab'.repeat(32),
    created_at: 1,
  }
}

/** The one breakpoint the shell branches on. */
function setViewport(width: number) {
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: width <= 760,
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    dispatchEvent: () => false,
  }))
}

beforeEach(() => {
  setViewport(1440)
  mutate.mockReset()
  dictations.length = 0
  useSpeaking.setState({ byScope: {}, spoken: {} })
  useComposerDraft.setState({ byScope: {} })
  vi.mocked(uploadArtifact).mockReset()
  vi.mocked(useSendMessage).mockReturnValue({ mutate } as unknown as ReturnType<
    typeof useSendMessage
  >)
})

function renderComposer(rootId?: string) {
  return render(<Composer api={{} as ApiClient} channelId="C1" rootId={rootId} />)
}

/** The composer form: the element that takes the dropped files. */
function composerForm(container: HTMLElement): HTMLFormElement {
  const form = container.querySelector('form')
  if (form === null) throw new Error('the composer has no form')
  return form
}

/** The handlers of the dictation the composer opened last. */
function handlers(): DictationHandlers {
  return dictations[dictations.length - 1].options.handlers
}

describe('Composer', () => {
  it('draws no frame on the field, which the card around it draws', () => {
    renderComposer()
    expect(screen.getByPlaceholderText('Message').className.split(' ')).toEqual([
      'ui-input',
      'ui-textarea',
      'ui-input-bare',
      'composer-input',
    ])
  })

  it('uploads a pasted file and sends its artifact id', async () => {
    vi.mocked(uploadArtifact).mockResolvedValue(dto('A1', 'shot.png'))
    renderComposer()
    const input = screen.getByPlaceholderText('Message')

    const file = new File(['png'], 'shot.png', { type: 'image/png' })
    fireEvent.paste(input, { clipboardData: { files: [file] } })

    // The chip appears once the upload settles.
    await waitFor(() => expect(screen.getByText('shot.png')).toBeTruthy())
    expect(uploadArtifact).toHaveBeenCalledWith(file)

    fireEvent.change(input, { target: { value: 'What is this?' } })
    fireEvent.keyDown(input, { key: 'Enter' })

    expect(mutate).toHaveBeenCalledWith(
      expect.objectContaining({
        text: 'What is this?',
        artifactIds: ['A1'],
      }),
    )
    // The composer clears after the send.
    expect(screen.queryByText('shot.png')).toBeNull()
  })

  it('uploads files picked through the attach button', async () => {
    vi.mocked(uploadArtifact).mockResolvedValue(dto('A2', 'notes.txt'))
    renderComposer()

    const file = new File(['text'], 'notes.txt', { type: 'text/plain' })
    fireEvent.change(screen.getByTestId('composer-file-input'), {
      target: { files: [file] },
    })

    await waitFor(() => expect(screen.getByText('notes.txt')).toBeTruthy())
    expect(uploadArtifact).toHaveBeenCalledWith(file)
  })

  // An http:// page on another machine is not a secure context, and
  // the browser defines no crypto.randomUUID there.
  it('sends on a page that is not a secure context', () => {
    const randomUUID = crypto.randomUUID
    Object.defineProperty(crypto, 'randomUUID', { value: undefined, configurable: true })
    try {
      renderComposer()
      const input = screen.getByPlaceholderText('Message')
      fireEvent.change(input, { target: { value: 'hello' } })

      fireEvent.keyDown(input, { key: 'Enter' })

      expect(mutate).toHaveBeenCalledWith(
        expect.objectContaining({
          text: 'hello',
          pendingId: expect.stringMatching(/^[0-9a-f-]{36}$/),
        }),
      )
    } finally {
      Object.defineProperty(crypto, 'randomUUID', { value: randomUUID, configurable: true })
    }
  })

  it('allows sending attachments without text', async () => {
    vi.mocked(uploadArtifact).mockResolvedValue(dto('A3', 'shot.png'))
    renderComposer()
    const input = screen.getByPlaceholderText('Message')
    const file = new File(['png'], 'shot.png', { type: 'image/png' })
    fireEvent.paste(input, { clipboardData: { files: [file] } })
    await waitFor(() => expect(screen.getByText('shot.png')).toBeTruthy())

    fireEvent.keyDown(input, { key: 'Enter' })

    expect(mutate).toHaveBeenCalledWith(
      expect.objectContaining({ text: '', artifactIds: ['A3'] }),
    )
  })

  it('shows the upload error and keeps the send disabled on failure', async () => {
    vi.mocked(uploadArtifact).mockRejectedValue(new Error('upload exceeds the limit'))
    renderComposer()
    const input = screen.getByPlaceholderText('Message')
    const file = new File(['big'], 'big.bin', { type: 'application/zip' })
    fireEvent.paste(input, { clipboardData: { files: [file] } })

    await waitFor(() =>
      expect(screen.getByText('upload exceeds the limit')).toBeTruthy(),
    )

    fireEvent.keyDown(input, { key: 'Enter' })
    expect(mutate).not.toHaveBeenCalled()
  })

  it('removes a chip without sending its artifact', async () => {
    vi.mocked(uploadArtifact).mockResolvedValue(dto('A4', 'shot.png'))
    renderComposer()
    const input = screen.getByPlaceholderText('Message')
    const file = new File(['png'], 'shot.png', { type: 'image/png' })
    fireEvent.paste(input, { clipboardData: { files: [file] } })
    await waitFor(() => expect(screen.getByText('shot.png')).toBeTruthy())

    fireEvent.click(screen.getByLabelText('Remove shot.png'))
    fireEvent.change(input, { target: { value: 'no attachment' } })
    fireEvent.keyDown(input, { key: 'Enter' })

    expect(mutate).toHaveBeenCalledWith(
      expect.objectContaining({ text: 'no attachment', artifactIds: [] }),
    )
  })
})

describe('Composer push-to-talk', () => {
  it('starts and releases dictation from the keyboard', () => {
    renderComposer()
    const mic = screen.getByLabelText('Hold to talk')
    fireEvent.keyDown(mic, { key: ' ' })
    expect(dictations).toHaveLength(1)
    fireEvent.keyDown(mic, { key: ' ', repeat: true })
    expect(dictations).toHaveLength(1)
    fireEvent.keyUp(mic, { key: ' ' })
    expect(dictations[0].released).toBe(1)
  })

  it('holds the mic, shows live text as pending, and lands the final transcript as a draft', () => {
    renderComposer()
    const mic = screen.getByLabelText('Hold to talk')
    const input = screen.getByPlaceholderText('Message') as HTMLTextAreaElement

    fireEvent.pointerDown(mic)
    expect(dictations).toHaveLength(1)
    expect(dictations[0].options.url).toMatch(/\/api\/v1\/channels\/C1\/dictate$/)
    expect(mic.getAttribute('aria-pressed')).toBe('true')

    act(() => {
      handlers().onReady(true)
      handlers().onDelta('book the')
      handlers().onDelta(' room')
    })
    // Pending text is shown and not editable, and the send waits.
    expect(input.value).toBe('book the room')
    expect(input.readOnly).toBe(true)
    expect((screen.getByText('Send') as HTMLButtonElement).disabled).toBe(true)

    fireEvent.pointerUp(mic)
    expect(dictations[0].released).toBe(1)
    expect(input.placeholder).toBe('Transcribing…')

    act(() => handlers().onFinal('book the room for tuesday'))
    expect(input.value).toBe('book the room for tuesday')
    expect(input.readOnly).toBe(false)

    // The user edits the draft and presses send: the same record a
    // typed message is.
    fireEvent.change(input, { target: { value: 'book the room for Tuesday' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(mutate).toHaveBeenCalledWith(
      expect.objectContaining({ text: 'book the room for Tuesday', artifactIds: [] }),
    )
  })

  it('a provider without live text says it is listening and appends the final transcript to the draft', () => {
    renderComposer()
    const input = screen.getByPlaceholderText('Message') as HTMLTextAreaElement
    fireEvent.change(input, { target: { value: 'Please' } })
    const mic = screen.getByLabelText('Hold to talk')

    fireEvent.pointerDown(mic)
    act(() => handlers().onReady(false))
    expect(input.placeholder).toBe('Listening…')
    expect(input.value).toBe('Please')

    fireEvent.pointerUp(mic)
    act(() => handlers().onFinal('book the room'))

    expect(input.value).toBe('Please book the room')
  })

  it('typing while held ends the dictation and keeps what was transcribed', () => {
    renderComposer()
    const mic = screen.getByLabelText('Hold to talk')
    const input = screen.getByPlaceholderText('Message') as HTMLTextAreaElement

    fireEvent.pointerDown(mic)
    act(() => handlers().onDelta('book'))
    fireEvent.keyDown(input, { key: 'a' })

    expect(dictations[0].released).toBe(1)
    act(() => handlers().onFinal('book'))
    expect(input.value).toBe('book')
  })

  it('shows the daemon error and keeps the draft', () => {
    renderComposer()
    const input = screen.getByPlaceholderText('Message') as HTMLTextAreaElement
    fireEvent.change(input, { target: { value: 'draft' } })

    fireEvent.pointerDown(screen.getByLabelText('Hold to talk'))
    act(() => handlers().onError('the provider is down'))

    expect(screen.getByTestId('dictation-error').textContent).toBe('the provider is down')
    expect(input.value).toBe('draft')
    expect(input.readOnly).toBe(false)
  })

  it('holding the mic turns speaking on for the thread', () => {
    renderComposer('root-1')

    fireEvent.pointerDown(screen.getByLabelText('Hold to talk'))

    expect(useSpeaking.getState().byScope).toEqual({ 'C1/root-1': true })
  })

  it('leaving the composer drops an utterance in progress', () => {
    const view = render(<Composer api={{} as ApiClient} channelId="C1" />)
    fireEvent.pointerDown(screen.getByLabelText('Hold to talk'))

    view.unmount()

    expect(dictations[0].canceled).toBe(1)
  })
})

describe('Composer drag and drop', () => {
  it('shows a drop zone on drag-over and drops a file as a chip', async () => {
    vi.mocked(uploadArtifact).mockResolvedValue(dto('A5', 'plan.pdf'))
    const view = renderComposer()
    const form = composerForm(view.container)

    expect(screen.queryByTestId('composer-dropzone')).toBeNull()
    fireEvent.dragOver(form)
    expect(screen.getByTestId('composer-dropzone').textContent).toBe(
      'Drop files to attach them',
    )

    const file = new File(['pdf'], 'plan.pdf', { type: 'application/pdf' })
    fireEvent.drop(form, { dataTransfer: { files: [file] } })

    expect(screen.queryByTestId('composer-dropzone')).toBeNull()
    await waitFor(() => expect(screen.getByText('plan.pdf')).toBeTruthy())
    expect(uploadArtifact).toHaveBeenCalledWith(file)
  })

  it('removing the chip of a dropped file keeps it out of the send', async () => {
    vi.mocked(uploadArtifact).mockResolvedValue(dto('A6', 'plan.pdf'))
    const view = renderComposer()
    const file = new File(['pdf'], 'plan.pdf', { type: 'application/pdf' })
    fireEvent.drop(composerForm(view.container), { dataTransfer: { files: [file] } })
    await waitFor(() => expect(screen.getByText('plan.pdf')).toBeTruthy())

    fireEvent.click(screen.getByLabelText('Remove plan.pdf'))
    expect(screen.queryByText('plan.pdf')).toBeNull()

    const input = screen.getByPlaceholderText('Message')
    fireEvent.change(input, { target: { value: 'no file' } })
    fireEvent.keyDown(input, { key: 'Enter' })

    expect(mutate).toHaveBeenCalledWith(
      expect.objectContaining({ text: 'no file', artifactIds: [] }),
    )
  })
})

describe('Composer keys', () => {
  it('sends on Enter', async () => {
    const user = userEvent.setup()
    renderComposer()
    const input = screen.getByPlaceholderText('Message') as HTMLTextAreaElement

    await user.click(input)
    await user.keyboard('ship it{Enter}')

    expect(mutate).toHaveBeenCalledWith(
      expect.objectContaining({ text: 'ship it', artifactIds: [] }),
    )
    expect(input.value).toBe('')
  })

  it('inserts a newline on Shift+Enter and sends nothing', async () => {
    const user = userEvent.setup()
    renderComposer()
    const input = screen.getByPlaceholderText('Message') as HTMLTextAreaElement

    await user.click(input)
    await user.keyboard('one{Shift>}{Enter}{/Shift}two')

    expect(input.value).toBe('one\ntwo')
    expect(mutate).not.toHaveBeenCalled()
  })
})

describe('Composer voice states', () => {
  /** The icon, the name and the state the voice control shows now. */
  function voiceControl(): { name: string; icon: string; state: string } {
    const button = screen.getByTestId('composer-mic')
    return {
      name: button.getAttribute('aria-label') ?? '',
      icon: button.querySelector('svg')?.getAttribute('class') ?? '',
      state: button.getAttribute('data-voice-state') ?? '',
    }
  }

  it('shows idle, listening and transcribing with a distinct icon and label', () => {
    renderComposer()
    const idle = voiceControl()
    expect(idle).toMatchObject({ name: 'Hold to talk', state: 'idle' })

    fireEvent.pointerDown(screen.getByTestId('composer-mic'))
    const listening = voiceControl()
    expect(listening).toMatchObject({ name: 'Listening', state: 'listening' })

    fireEvent.pointerUp(screen.getByTestId('composer-mic'))
    const transcribing = voiceControl()
    expect(transcribing).toMatchObject({ name: 'Transcribing', state: 'transcribing' })

    // Each state carries its own icon.
    expect(new Set([idle.icon, listening.icon, transcribing.icon]).size).toBe(3)

    act(() => handlers().onFinal('done'))
    expect(voiceControl()).toMatchObject({ name: 'Hold to talk', state: 'idle' })
  })
})

describe('Composer menus', () => {
  it('opens the attach menu and picks files through it', async () => {
    const user = userEvent.setup()
    renderComposer()
    const click = vi.spyOn(HTMLInputElement.prototype, 'click')

    await user.click(screen.getByRole('button', { name: 'Attach' }))
    await user.click(await screen.findByRole('menuitem', { name: 'Attach files' }))

    expect(click).toHaveBeenCalled()
    click.mockRestore()
  })

  it('opens the command menu on a leading slash and runs a command', async () => {
    const user = userEvent.setup()
    renderComposer()
    const input = screen.getByPlaceholderText('Message') as HTMLTextAreaElement

    await user.click(input)
    await user.keyboard('/')

    const names = screen.getAllByRole('menuitem').map((item) => item.textContent ?? '')
    expect(names.some((name) => name.startsWith('Attach files'))).toBe(true)
    expect(names.some((name) => name.startsWith('Talk'))).toBe(true)

    // The query filters, and Enter runs the highlighted command.
    await user.keyboard('talk{Enter}')
    expect(dictations).toHaveLength(1)
    expect(input.value).toBe('')
  })

  it('closes the command menu on Escape', async () => {
    const user = userEvent.setup()
    renderComposer()
    await user.click(screen.getByPlaceholderText('Message'))
    await user.keyboard('/')
    expect(screen.getByTestId('composer-commands')).toBeTruthy()

    await user.keyboard('{Escape}')
    expect(screen.queryByTestId('composer-commands')).toBeNull()
  })
})

describe('Composer control names and tooltips', () => {
  it('names every control and explains it in a tooltip', async () => {
    const user = userEvent.setup()
    renderComposer()
    // A draft enables the send control, so its tooltip can be reached.
    fireEvent.change(screen.getByPlaceholderText('Message'), {
      target: { value: 'hello' },
    })

    const controls: [string, string][] = [
      ['Attach', 'Attach'],
      ['Hold to talk', 'Hold to talk'],
      ['Send', 'Send the message (Enter)'],
    ]
    for (const [name, tooltip] of controls) {
      const control = screen.getByRole('button', { name })
      await user.hover(control)
      await waitFor(() =>
        expect(screen.getAllByRole('tooltip').map((node) => node.textContent)).toContain(
          tooltip,
        ),
      )
      await user.unhover(control)
    }
  })

  it('takes the drafted first message of its scope and lets the user edit it', async () => {
    useComposerDraft.getState().set('C1', 'Please set up a schedule for me.')
    renderComposer()

    const box = await screen.findByPlaceholderText('Message')
    expect((box as HTMLTextAreaElement).value).toBe(
      'Please set up a schedule for me.',
    )
    // The draft is the composer's now: it never lands a second time.
    expect(useComposerDraft.getState().byScope['C1']).toBeUndefined()

    fireEvent.change(box, { target: { value: 'Please set up a schedule at 09:00.' } })
    expect((box as HTMLTextAreaElement).value).toBe(
      'Please set up a schedule at 09:00.',
    )
  })

  // Home writes into the Chief of Staff's Channel from its own page
  // (ADR-0022), so the draft written for that Channel's Thread has to
  // reach the Thread, not Home.
  it('leaves the draft alone when it only reaches into the channel', () => {
    useComposerDraft.getState().set('C1', 'Please call +14155550199 back.')
    render(
      <Composer api={{} as ApiClient} channelId="C1" adoptsDraft={false} />,
    )

    expect(
      (screen.getByPlaceholderText('Message') as HTMLTextAreaElement).value,
    ).toBe('')
    expect(useComposerDraft.getState().byScope['C1']).toBe(
      'Please call +14155550199 back.',
    )
  })

  it('a draft for another scope stays out of this composer', () => {
    useComposerDraft.getState().set('C2', 'not for this channel')
    renderComposer()

    expect(
      (screen.getByPlaceholderText('Message') as HTMLTextAreaElement).value,
    ).toBe('')
    expect(useComposerDraft.getState().byScope['C2']).toBe('not for this channel')
  })
})

// The phone composer uses icons so it fits a narrow screen.
describe('the composer at the phone width', () => {
  it('uses icon controls and replaces dictation with Send for a draft', () => {
    setViewport(375)
    renderComposer()
    const row = screen.getByTestId('composer-mic').parentElement
    if (row === null) throw new Error('the mic sits in no row')
    const worded = [...row.querySelectorAll('button')]
      .map((button) => button.textContent?.trim() ?? '')
      .filter((words) => words !== '')
    expect(worded).toEqual([])
    expect(screen.getByRole('button', { name: 'Dictate a message' })).toBeTruthy()
    fireEvent.change(screen.getByPlaceholderText('Message'), { target: { value: 'Book the trip' } })
    expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Dictate a message' })).toBeNull()
  })
})
