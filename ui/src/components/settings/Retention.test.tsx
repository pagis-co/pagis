// The Retention section: one row per artifact class.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { Retention } from './Retention'

const policies = [
  { kind: 'screenshot', retain_days: null },
  { kind: 'call_recording', retain_days: 30 },
  { kind: 'call_transcript', retain_days: null },
  { kind: 'file', retain_days: null },
]

function stubApi() {
  return {
    GET: vi.fn(async () => ({ data: { items: policies } })),
    PUT: vi.fn(async () => ({ data: { kind: 'screenshot', retain_days: 90 } })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <Retention api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

describe('Retention', () => {
  it('shows the title line, one row per class, and an empty box means keep for ever', async () => {
    const api = stubApi()
    mount(api)

    expect(await screen.findByText('Screenshots')).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Retention' })).toBeTruthy()
    expect(screen.getByText(/Empty means for ever/)).toBeTruthy()
    expect(screen.getByText('Call recordings')).toBeTruthy()
    expect(screen.getByText('Call transcripts')).toBeTruthy()
    expect(screen.getByText('Files')).toBeTruthy()
    expect(screen.getAllByText('days')).toHaveLength(4)

    const screenshots = screen.getByLabelText('Days to keep Screenshots') as HTMLInputElement
    expect(screenshots.value).toBe('')
    expect(screenshots.placeholder).toBe('Keep for ever')
    const recordings = screen.getByLabelText(
      'Days to keep Call recordings',
    ) as HTMLInputElement
    expect(recordings.value).toBe('30')
  })

  it('says that memory is not here', async () => {
    mount(stubApi())
    expect(await screen.findByText(/Memory is not here/)).toBeTruthy()
  })

  it('saves a window for one class', async () => {
    const api = stubApi()
    mount(api)

    const input = (await screen.findByLabelText(
      'Days to keep Screenshots',
    )) as HTMLInputElement
    fireEvent.change(input, { target: { value: '90' } })
    fireEvent.click(screen.getByLabelText('Save the window for Screenshots'))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/retention/{kind}', {
      params: { path: { kind: 'screenshot' } },
      body: { retain_days: 90 },
    })
  })

  it('clears a window back to keep for ever', async () => {
    const api = stubApi()
    mount(api)

    const input = (await screen.findByLabelText(
      'Days to keep Call recordings',
    )) as HTMLInputElement
    fireEvent.change(input, { target: { value: '' } })
    fireEvent.click(screen.getByLabelText('Save the window for Call recordings'))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/retention/{kind}', {
      params: { path: { kind: 'call_recording' } },
      body: { retain_days: null },
    })
  })

  it('does not save a window under one day', async () => {
    const api = stubApi()
    mount(api)

    const input = (await screen.findByLabelText('Days to keep Files')) as HTMLInputElement
    fireEvent.change(input, { target: { value: '0' } })
    const save = screen.getByLabelText('Save the window for Files') as HTMLButtonElement
    expect(save.disabled).toBe(true)
    fireEvent.click(save)
    expect(api.PUT).not.toHaveBeenCalled()
  })

  it('shows the refusal on the row that failed', async () => {
    const api = stubApi()
    api.PUT.mockResolvedValueOnce({
      data: undefined,
      error: { message: 'The daemon refused the window.' },
    } as never)
    mount(api)

    const input = (await screen.findByLabelText('Days to keep Files')) as HTMLInputElement
    fireEvent.change(input, { target: { value: '7' } })
    fireEvent.click(screen.getByLabelText('Save the window for Files'))

    expect(await screen.findByRole('alert')).toBeTruthy()
  })
})
