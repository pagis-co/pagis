// The sprite voice picker offers the Provider Voice List: the voices of
// the model that speaks, read from the daemon.

import { useState } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { VoicePicker } from './VoicePicker'

type Voice = { id: string; name: string | null }
type Page = { provider: string | null; model: string | null; items: Voice[] }

const GEMINI: Page = {
  provider: 'openrouter',
  model: 'google/gemini-3.8-flash-tts',
  items: [
    { id: 'Zephyr', name: null },
    { id: 'Kore', name: null },
  ],
}

const ELEVENLABS: Page = {
  provider: 'elevenlabs',
  model: 'eleven_flash_v2_5',
  items: [
    { id: '21m00Tcm4TlvDq8ikWAM', name: 'Rachel' },
    { id: 'EXAVITQu4vr4xnSDxMaL', name: 'Sarah' },
  ],
}

function Harness({ page, held }: { page: Page; held: string }) {
  const [voice, setVoice] = useState(held)
  const api = { GET: vi.fn(async () => ({ data: page })) }
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return (
    <QueryClientProvider client={queryClient}>
      <VoicePicker api={api as unknown as ApiClient} value={voice} onChange={setVoice} />
      <output aria-label="Chosen voice">{voice}</output>
    </QueryClientProvider>
  )
}

function picker() {
  return screen.getByRole('combobox', { name: 'Sprite voice' })
}

describe('VoicePicker', () => {
  it('names the default voice of the model that speaks and offers its voices', async () => {
    const user = userEvent.setup()
    render(<Harness page={GEMINI} held="" />)

    expect(await screen.findByText('Default voice (Zephyr)')).toBeTruthy()
    await user.click(picker())
    await user.click(await screen.findByRole('option', { name: 'Kore' }))

    expect(screen.getByLabelText('Chosen voice').textContent).toBe('Kore')
  })

  it('shows the name of a voice and holds its id', async () => {
    const user = userEvent.setup()
    render(<Harness page={ELEVENLABS} held="" />)

    expect(await screen.findByText('Default voice (Rachel)')).toBeTruthy()
    await user.click(picker())
    await user.click(await screen.findByRole('option', { name: 'Sarah' }))

    expect(screen.getByLabelText('Chosen voice').textContent).toBe('EXAVITQu4vr4xnSDxMaL')
  })

  it('keeps a held voice the model does not offer and says so', async () => {
    render(<Harness page={GEMINI} held="nova" />)

    expect(
      await screen.findByText('nova (not a voice of google/gemini-3.8-flash-tts)'),
    ).toBeTruthy()
  })

  it('offers no voice when no key serves spoken replies', async () => {
    render(<Harness page={{ provider: null, model: null, items: [] }} held="" />)

    expect(await screen.findByText('No key serves spoken replies')).toBeTruthy()
    expect((picker() as HTMLButtonElement).disabled).toBe(true)
  })
})
