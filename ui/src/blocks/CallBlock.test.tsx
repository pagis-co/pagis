// The `call` block (ADR-0022): one card, the head while the
// Call runs and the whole record after it — the face, the number, the
// tier, the state chip, the duration, the recording and the transcript.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient, CallDto } from '../api/client'
import { useCallInspector, useCallTranscripts } from '../state/stores'
import { CallBlock } from './CallBlock'

vi.mock('../api/client', async () => {
  const actual = await vi.importActual<typeof import('../api/client')>(
    '../api/client',
  )
  return {
    ...actual,
    fetchArtifactBlob: vi.fn(async () => new Blob(['wav'])),
  }
})

const live: CallDto = {
  id: 'call_1',
  agent_id: 'agent_1',
  agent_name: 'Robin',
  own_e164: '+14155550123',
  direction: 'outbound',
  remote_e164: '+14155559981',
  purpose: 'Move the cleaning to a morning.',
  tools: ['memory_read'],
  tier: 'unknown',
  state: 'live',
  outcome: null,
  ended_reason: null,
  classification: null,
  message_left: false,
  transcript: [{ at: 1_000, speaker: 'agent', text: 'Hello, this is Robin.' }],
  recording_artifact_id: null,
  created_at: 0,
  answered_at: 1_000,
  ended_at: null,
} as CallDto

const settled: CallDto = {
  ...live,
  state: 'ended',
  outcome: 'answered',
  ended_reason: 'agent_hangup',
  classification: 'human',
  recording_artifact_id: 'art_1',
  transcript: [
    { at: 1_000, speaker: 'agent', text: 'Hello, this is Robin.' },
    { at: 2_000, speaker: 'caller', text: 'Wednesday at nine twenty.' },
  ],
  ended_at: 61_000,
} as CallDto

// The failed fixture: the carrier refused the call, so nothing was
// answered and nothing was said.
const failedCall: CallDto = {
  ...live,
  state: 'ended',
  outcome: 'failed',
  ended_reason: 'refused',
  classification: null,
  answered_at: null,
  ended_at: 4_000,
  transcript: [],
} as CallDto

function stubApi(call: CallDto | null) {
  return {
    GET: vi.fn(async () =>
      call === null ? { error: { error: { message: 'gone' } } } : { data: call },
    ),
    POST: vi.fn(async () => ({ data: {} })),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({ data: {} })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <CallBlock callId="call_1" api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  useCallInspector.setState({ callId: null })
  useCallTranscripts.setState({ byCall: {} })
})

describe('the strip', () => {
  it('is one line: the direction, who is on the call, and the last thing said', async () => {
    mount(stubApi(live))
    const strip = await screen.findByTestId('call-strip')
    expect(strip.textContent).toContain('Call to +1 415 555 9981')
    expect(strip.textContent).toContain('Hello, this is Robin.')
    expect(strip.textContent).toContain('Unknown')
  })

  it('follows the `call.transcript` events', async () => {
    mount(stubApi(live))
    await screen.findByTestId('call-strip')
    useCallTranscripts
      .getState()
      .append('call_1', { at: 2_000, speaker: 'caller', text: 'Wednesday works.' })
    await waitFor(() =>
      expect(screen.getByTestId('call-strip').textContent).toContain(
        'Wednesday works.',
      ),
    )
  })

  it('is the way back to the call: it opens the inspector', async () => {
    mount(stubApi(live))
    fireEvent.click(await screen.findByTestId('call-strip'))
    expect(useCallInspector.getState().callId).toBe('call_1')
  })

  it('says the call is not on record when the daemon has none', async () => {
    mount(stubApi(null))
    await waitFor(() =>
      expect(screen.getByTestId('call-waiting').textContent).toContain(
        'That call is not on record.',
      ),
    )
  })
})

describe('the state chip', () => {
  it('says Ringing before the call is answered', async () => {
    mount(stubApi({ ...live, answered_at: null } as CallDto))
    const strip = await screen.findByTestId('call-strip')
    expect(strip.textContent).toContain('Ringing')
    expect(strip.querySelector('.ui-badge-waiting')).not.toBeNull()
  })

  it('says Connected in the on-call hue while the call runs', async () => {
    mount(stubApi(live))
    const strip = await screen.findByTestId('call-strip')
    expect(strip.textContent).toContain('Connected')
    expect(strip.querySelector('.ui-badge-on-call')).not.toBeNull()
  })

  it('says Ended on a call that ran and finished', async () => {
    mount(stubApi(settled))
    expect((await screen.findByTestId('call-strip')).textContent).toContain(
      'Ended',
    )
  })

  it('says Failed in the failed hue on a call that never went through', async () => {
    mount(stubApi(failedCall))
    const strip = await screen.findByTestId('call-strip')
    expect(strip.textContent).toContain('Failed')
    expect(strip.querySelector('.ui-badge-failed')).not.toBeNull()
  })
})

describe('a failed call', () => {
  it('says why it failed in plain words, inside the card', async () => {
    mount(stubApi(failedCall))
    const card = await screen.findByTestId('call-settled')
    expect(card.textContent).toContain(
      'This call did not go through: the carrier refused the call.',
    )
  })
})

describe('the card head', () => {
  it('carries the agent face, the number, the tier and the duration', async () => {
    mount(stubApi(settled))
    const strip = await screen.findByTestId('call-strip')
    expect(strip.querySelector('.ui-avatar [role=img]')?.getAttribute('aria-label')).toBe('Robin, Pixie avatar')
    expect(strip.textContent).toContain('+1 415 555 9981')
    expect(strip.textContent).toContain('Unknown')
    expect(strip.textContent).toContain('1:00')
  })
})

describe('the settled block', () => {
  it('keeps the outcome apart from the reason the call ended', async () => {
    mount(stubApi(settled))
    const block = await screen.findByTestId('call-settled')
    expect(block.textContent).toContain('Answered')
    expect(block.textContent).toContain('Ended because the sprite hung up')
    expect(block.textContent).toContain('1:00')
    expect(block.textContent).toContain('a person answered')
  })

  it('says nothing about the classification of a call the agent answered', async () => {
    // The classify phase runs on an outbound call alone, so an
    // inbound call carries no verdict and the card claims none
    // (ADR-0020).
    mount(
      stubApi({
        ...settled,
        direction: 'inbound',
        classification: null,
      } as CallDto),
    )
    const block = await screen.findByTestId('call-settled')
    expect(block.textContent).toContain('Ended because the sprite hung up')
    expect(block.textContent).not.toContain('nothing classified the answer')
  })

  it('draws its own scrubber for the recording, with elapsed and total time', async () => {
    mount(stubApi(settled))
    const slider = await screen.findByRole('slider', {
      name: 'The recording of the call',
    })
    expect(slider.getAttribute('aria-valuetext')).toBe('0:00 / 1:00')
  })

  it('seeks the recording with the arrow keys', async () => {
    mount(stubApi(settled))
    const slider = await screen.findByRole('slider', {
      name: 'The recording of the call',
    })
    slider.focus()
    fireEvent.keyDown(slider, { key: 'ArrowRight' })
    fireEvent.keyDown(slider, { key: 'ArrowRight' })
    expect(slider.getAttribute('aria-valuenow')).toBe('10')
    expect(slider.getAttribute('aria-valuetext')).toBe('0:10 / 1:00')
    fireEvent.keyDown(slider, { key: 'ArrowLeft' })
    expect(slider.getAttribute('aria-valuenow')).toBe('5')
    fireEvent.keyDown(slider, { key: 'End' })
    expect(slider.getAttribute('aria-valuenow')).toBe('60')
    fireEvent.keyDown(slider, { key: 'Home' })
    expect(slider.getAttribute('aria-valuenow')).toBe('0')
  })

  it('toggles a short transcript inline, inside the same card', async () => {
    mount(stubApi(settled))
    const toggle = await screen.findByRole('button', {
      name: 'Read the transcript',
    })
    expect(screen.queryByTestId('call-transcript')).toBeNull()
    fireEvent.click(toggle)
    expect(screen.getByTestId('call-transcript').textContent).toContain(
      'Wednesday at nine twenty.',
    )
    expect(
      screen
        .getByTestId('call-settled')
        .contains(screen.getByTestId('call-transcript')),
    ).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Hide the transcript' }))
    expect(screen.queryByTestId('call-transcript')).toBeNull()
  })

  it('sends a long transcript to the inspector rather than the Thread', async () => {
    const long = {
      ...settled,
      transcript: Array.from({ length: 20 }, (_, index) => ({
        at: index,
        speaker: 'caller',
        text: `line ${index}`,
      })),
    } as CallDto
    mount(stubApi(long))
    fireEvent.click(
      await screen.findByRole('button', { name: 'Read the transcript (20 lines)' }),
    )
    expect(useCallInspector.getState().callId).toBe('call_1')
  })
})

describe('an inbound call', () => {
  it('says the Agent answered its desk line under the standing brief', async () => {
    mount(stubApi({ ...live, direction: 'inbound' } as CallDto))
    expect((await screen.findByTestId('call-block')).textContent).toContain(
      'Robin answered its desk line +1 415 555 0123 under its standing brief.',
    )
  })
})
