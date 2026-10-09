// A live Call on the phone: who calls, the state and the tier, the
// transcript and the tools of the Call, and the three acts on it. Hang
// up and a drop of the tier each ask first in an action sheet.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient, CallDto } from '../../api/client'
import { formatE164 } from '../../blocks/call'
import { shellResponse } from '../../test/appStub'
import { renderInRouter } from '../../test/router'
import { CallPhone } from './CallPhone'

// Listening opens a socket to the call audio. The stand-in answers at
// once, as the socket does when the audio is ready.
vi.mock('../../blocks/ListenLive', () => ({
  useListenLive: () => {
    const [state, setState] = useState<'idle' | 'listening'>('idle')
    return { state, message: null, start: () => setState('listening'), stop: () => setState('idle') }
  },
}))

let call: CallDto

function mount() {
  const post = vi.fn(async (_path: string) => ({ data: { ok: true } }))
  const api = {
    GET: vi.fn(async (path: string) => (path === '/api/v1/calls/{call_id}' ? { data: call } : shellResponse(path))),
    POST: post,
  } as unknown as ApiClient
  renderInRouter(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <CallPhone api={api} callId="call-1" />
    </QueryClientProvider>,
  )
  return { post }
}

beforeEach(() => {
  call = {
    ...(shellResponse('/api/v1/calls/{call_id}').data as CallDto),
    id: 'call-1',
    direction: 'inbound',
    remote_e164: '+14155550199',
    tier: 'trusted',
    state: 'live',
    created_at: Date.now() - 60_000,
    answered_at: Date.now() - 50_000,
    tools: ['calendar__read', 'mail__send'],
    transcript: [{ at: 1, speaker: 'caller', text: 'Is the table booked?' }],
  }
})

describe('CallPhone', () => {
  it('shows who calls, the state and the tier of a live inbound Call', async () => {
    mount()

    expect(await screen.findByRole('heading', { name: formatE164('+14155550199') })).toBeTruthy()
    expect(screen.getByText('Sage · Call from')).toBeTruthy()
    expect(screen.getByText('Connected')).toBeTruthy()
    expect(screen.getByText('Trusted')).toBeTruthy()
  })

  it('lists the tools of the Call', async () => {
    mount()

    expect(await screen.findByText('Tools on this call: calendar__read, mail__send')).toBeTruthy()
  })

  it('hangs up after the person confirms', async () => {
    const { post } = mount()

    fireEvent.click(await screen.findByRole('button', { name: 'Hang up' }))
    const sheet = await screen.findByRole('alertdialog', { name: 'Hang up this call?' })
    expect(post).not.toHaveBeenCalled()
    fireEvent.click(within(sheet).getByRole('button', { name: 'Hang up' }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post.mock.calls[0][0]).toBe('/api/v1/calls/{call_id}/hangup')
  })

  it('confirms a drop to Unknown before it happens', async () => {
    const { post } = mount()

    fireEvent.click(await screen.findByRole('button', { name: 'Drop to Unknown' }))
    const sheet = await screen.findByRole('alertdialog', { name: 'Drop this call to Unknown?' })
    fireEvent.click(within(sheet).getByRole('button', { name: 'Drop to Unknown' }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post.mock.calls[0][0]).toBe('/api/v1/calls/{call_id}/tier')
  })

  it('offers no drop on an Unknown Call', async () => {
    call = { ...call, tier: 'unknown' }
    mount()

    await screen.findByRole('button', { name: 'Hang up' })
    expect(screen.queryByRole('button', { name: 'Drop to Unknown' })).toBeNull()
  })

  it('turns Listen into Stop listening', async () => {
    mount()

    fireEvent.click(await screen.findByRole('button', { name: 'Listen' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Stop listening' }))

    expect(await screen.findByRole('button', { name: 'Listen' })).toBeTruthy()
  })
})
