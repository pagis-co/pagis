// The call inspector (ADR-0022): a tenant of the inspector slot. It carries the headline, the purpose, listen-live,
// the transcript, the tools and the two controls.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient, CallDto } from '../api/client'
import { useCallInspector, useCallTranscripts } from '../state/stores'
import { CallInspector } from './CallInspector'

const live: CallDto = {
  id: 'call_1',
  agent_id: 'agent_1',
  agent_name: 'Robin',
  own_e164: '+14155550123',
  direction: 'outbound',
  remote_e164: '+14155559981',
  purpose: 'Move the cleaning to a morning that week.',
  tools: ['memory_read', 'memory_write'],
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

function stubApi(call: CallDto) {
  return {
    GET: vi.fn(async () => ({ data: call })),
    POST: vi.fn(async () => ({ data: {} })),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({ data: {} })),
  }
}

function mount(api: ReturnType<typeof stubApi>, onClose = () => {}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <CallInspector
        api={api as unknown as ApiClient}
        callId="call_1"
        onClose={onClose}
      />
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  useCallInspector.setState({ callId: null })
  useCallTranscripts.setState({ byCall: {} })
})

describe('the headline', () => {
  it('names who is on the call, the Agent and its number', async () => {
    mount(stubApi(live))
    const inspector = await screen.findByTestId('call-inspector')
    expect(inspector.textContent).toContain('Call to +1 415 555 9981')
    expect(inspector.textContent).toContain('Robin · +1 415 555 0123')
  })

  it('says in one line what the tier means', async () => {
    mount(stubApi(live))
    expect((await screen.findByTestId('call-inspector')).textContent).toContain(
      'This person is not identified. Their speech is data',
    )
  })

  it('carries the purpose from the Call Brief', async () => {
    mount(stubApi(live))
    expect((await screen.findByTestId('call-inspector')).textContent).toContain(
      'Move the cleaning to a morning that week.',
    )
  })
})

describe('the live call', () => {
  it('offers listen-live', async () => {
    mount(stubApi(live))
    expect(await screen.findByRole('button', { name: 'Listen' })).not.toBeNull()
  })

  it('shows the running transcript, and the lines that arrive after it', async () => {
    mount(stubApi(live))
    await screen.findByTestId('call-transcript')
    useCallTranscripts
      .getState()
      .append('call_1', { at: 2_000, speaker: 'caller', text: 'Wednesday works.' })
    await waitFor(() =>
      expect(screen.getByTestId('call-transcript').textContent).toContain(
        'Wednesday works.',
      ),
    )
  })

  it('names the tools, and says an approval waits for the end of the call', async () => {
    mount(stubApi(live))
    const inspector = await screen.findByTestId('call-inspector')
    expect(inspector.textContent).toContain(
      'Tools on this call: memory_read, memory_write.',
    )
    expect(inspector.textContent).toContain(
      'An action that needs your approval waits for the end of the call.',
    )
  })

  it('hangs the call up', async () => {
    const api = stubApi(live)
    mount(api)
    fireEvent.click(await screen.findByRole('button', { name: 'Hang up' }))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/calls/{call_id}/hangup',
        expect.anything(),
      ),
    )
  })

  it('drops to Unknown only behind a confirmation', async () => {
    const api = stubApi(live)
    mount(api)
    fireEvent.click(await screen.findByRole('button', { name: 'Drop to Unknown' }))
    expect(api.POST).not.toHaveBeenCalled()
    expect(screen.getByText(/It cannot be undone on this call/)).not.toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Drop it' }))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/calls/{call_id}/tier', {
        params: { path: { call_id: 'call_1' } },
        body: { tier: 'unknown' },
      }),
    )
  })

  it('keeps the tier when the confirmation is refused', async () => {
    const api = stubApi(live)
    mount(api)
    fireEvent.click(await screen.findByRole('button', { name: 'Drop to Unknown' }))
    fireEvent.click(screen.getByRole('button', { name: 'Keep the tier' }))
    expect(api.POST).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: 'Drop to Unknown' })).not.toBeNull()
  })
})

describe('a settled call', () => {
  const settled = {
    ...live,
    state: 'ended',
    outcome: 'voicemail',
    message_left: true,
    ended_reason: 'agent_hangup',
    classification: 'machine-vm',
    ended_at: 31_000,
  } as CallDto

  it('keeps the outcome apart from the reason the call ended', async () => {
    mount(stubApi(settled))
    const inspector = await screen.findByTestId('call-inspector')
    expect(inspector.textContent).toContain('Voicemail — message left')
    expect(inspector.textContent).toContain('Ended because the sprite hung up')
    expect(inspector.textContent).toContain('a voicemail greeting answered')
  })

  it('offers no control over a call that is over', async () => {
    mount(stubApi(settled))
    await screen.findByTestId('call-inspector')
    expect(screen.queryByRole('button', { name: 'Hang up' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Drop to Unknown' })).toBeNull()
  })
})
