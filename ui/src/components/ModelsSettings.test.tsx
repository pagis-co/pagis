// The Models section: aliases as ordered candidate chips with a
// note each.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { ModelsSettings } from './ModelsSettings'

function stubApi() {
  return {
    GET: vi.fn(async (path: string) => {
      switch (path) {
        case '/api/v1/settings/model-aliases':
          return {
            data: {
              items: [
                {
                  alias: 'default',
                  candidates: ['anthropic/claude-sonnet-4-6', 'openai/gpt-5'],
                  settings: [],
                  updated_at: 1,
                },
                {
                  alias: 'transcribe',
                  candidates: ['openai/gpt-4o-transcribe'],
                  settings: [],
                  updated_at: 1,
                },
                {
                  alias: 'phone',
                  candidates: ['openai/gpt-live-1'],
                  settings: [{
                    alias: 'gpt-live-reasoning',
                    label: 'GPT-Live reasoning model',
                    description: 'The Responses model that reasons and selects tools for GPT-Live.',
                    when_candidates: ['openai/gpt-live-1'],
                    candidates: ['openai/gpt-5.6-terra'],
                  }],
                  updated_at: 1,
                },
                {
                  alias: 'gpt-live-reasoning',
                  candidates: ['openai/gpt-5.6-terra'],
                  settings: [],
                  updated_at: 1,
                },
                {
                  alias: 'draft',
                  candidates: ['openai/gpt-5-mini'],
                  settings: [],
                  updated_at: 1,
                },
              ],
            },
          }
        case '/api/v1/settings/models':
          return {
            data: {
              providers: [
                {
                  provider: 'openai',
                  error: null,
                  models: [
                    {
                      candidate: 'openai/gpt-6-luna',
                      context_window: 128000,
                      max_output_tokens: 4096,
                      input_cost: null,
                      output_cost: null,
                    },
                  ],
                },
              ],
            },
          }
        default:
          throw new Error(`unexpected GET ${path}`)
      }
    }),
    POST: vi.fn(async () => ({ data: {} })),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({ data: undefined, response: { ok: true } })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <ModelsSettings api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

describe('ModelsSettings', () => {
  it('lists each alias as ordered chips with a note', async () => {
    mount(stubApi())

    const row = (await screen.findByText('default')).closest('.ui-row') as HTMLElement
    const chips = within(row).getAllByRole('listitem').map((chip) => chip.textContent)
    expect(chips).toEqual(['1 · anthropic/claude-sonnet-4-6', '2 · openai/gpt-5'])
    expect(within(row).getByText('every seeded sprite')).toBeTruthy()

    const transcribe = screen.getByText('transcribe').closest('.ui-row') as HTMLElement
    expect(within(transcribe).getByText('buffered speech to text')).toBeTruthy()
    const phone = screen.getByText('phone').closest('.ui-row') as HTMLElement
    expect(within(phone).getByText('telephone conversations')).toBeTruthy()
    // An alias the product does not know carries no note.
    const draft = screen.getByText('draft').closest('.ui-row') as HTMLElement
    expect(within(draft).getAllByRole('listitem')).toHaveLength(1)

  })

  it('edits the candidates of an alias, and deletes the alias', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Edit default' }))
    const dialog = await screen.findByRole('dialog', { name: 'Edit default' })
    expect(within(dialog).getByText('OpenRouter candidates use openrouter/provider/model.')).toBeTruthy()
    const candidates = within(dialog).getByLabelText('Candidates, one per line')
    expect((candidates as HTMLTextAreaElement).value).toBe(
      'anthropic/claude-sonnet-4-6\nopenai/gpt-5',
    )
    fireEvent.change(candidates, { target: { value: 'openai/gpt-5\n\nanthropic/claude-sonnet-4-6\n' } })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/model-aliases/{alias}', {
        params: { path: { alias: 'default' } },
        body: { candidates: ['openai/gpt-5', 'anthropic/claude-sonnet-4-6'] },
      }),
    )
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())

    fireEvent.click(screen.getByRole('button', { name: 'Edit draft' }))
    const draft = await screen.findByRole('dialog', { name: 'Edit draft' })
    fireEvent.click(within(draft).getByRole('button', { name: 'Delete' }))
    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/model-aliases/{alias}', {
        params: { path: { alias: 'draft' } },
      }),
    )
  })

  it('offers the listed models as candidates and still takes a typed id', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Edit default' }))
    const dialog = await screen.findByRole('dialog', { name: 'Edit default' })
    const adding = within(dialog).getByRole('combobox', { name: 'Add a candidate' })
    fireEvent.click(adding)
    expect(
      (await screen.findAllByRole('option')).map((option) => option.textContent),
    ).toEqual(['openai/gpt-6-luna'])

    fireEvent.change(adding, { target: { value: 'openai/gpt-6-luna' } })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add candidate' }))
    fireEvent.change(adding, { target: { value: 'openrouter/vendor/typed-model' } })
    fireEvent.keyDown(adding, { key: 'Enter' })

    const candidates = within(dialog).getByLabelText('Candidates, one per line')
    expect((candidates as HTMLTextAreaElement).value).toBe(
      'anthropic/claude-sonnet-4-6\nopenai/gpt-5\nopenai/gpt-6-luna\nopenrouter/vendor/typed-model',
    )
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/model-aliases/{alias}', {
        params: { path: { alias: 'default' } },
        body: {
          candidates: [
            'anthropic/claude-sonnet-4-6',
            'openai/gpt-5',
            'openai/gpt-6-luna',
            'openrouter/vendor/typed-model',
          ],
        },
      }),
    )
  })

  it('configures the reasoning model only while GPT-Live is a phone candidate', async () => {
    const api = stubApi()
    mount(api)

    await screen.findByText('phone')
    expect(screen.queryByText('gpt-live-reasoning')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Edit phone' }))
    const dialog = await screen.findByRole('dialog', { name: 'Edit phone' })
    const phoneCandidates = within(dialog).getByLabelText('Candidates, one per line')
    const reasoning = within(dialog).getByLabelText('GPT-Live reasoning model')
    expect((reasoning as HTMLTextAreaElement).value).toBe('openai/gpt-5.6-terra')

    fireEvent.change(phoneCandidates, { target: { value: 'openai/gpt-realtime-2.1' } })
    expect(within(dialog).queryByLabelText('GPT-Live reasoning model')).toBeNull()

    fireEvent.change(phoneCandidates, { target: { value: 'openai/gpt-live-1' } })
    const restored = within(dialog).getByLabelText('GPT-Live reasoning model')
    fireEvent.change(restored, { target: { value: 'openai/gpt-5.6-sol' } })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalledTimes(2))
    expect(api.PUT).toHaveBeenNthCalledWith(1, '/api/v1/settings/model-aliases/{alias}', {
      params: { path: { alias: 'phone' } },
      body: { candidates: ['openai/gpt-live-1'] },
    })
    expect(api.PUT).toHaveBeenNthCalledWith(2, '/api/v1/settings/model-aliases/{alias}', {
      params: { path: { alias: 'gpt-live-reasoning' } },
      body: { candidates: ['openai/gpt-5.6-sol'] },
    })
  })

  it('adds an alias from the title line', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Add an alias' }))
    const dialog = await screen.findByRole('dialog', { name: 'Add an alias' })
    const add = within(dialog).getByRole('button', { name: 'Add' })
    expect(add).toHaveProperty('disabled', true)
    fireEvent.change(within(dialog).getByLabelText('Name'), { target: { value: ' fast ' } })
    fireEvent.change(within(dialog).getByLabelText('Candidates, one per line'), {
      target: { value: 'openai/gpt-5-mini' },
    })
    fireEvent.click(add)
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/model-aliases', {
        body: { alias: 'fast', candidates: ['openai/gpt-5-mini'] },
      }),
    )
  })

  // The provider keys belong to the Org and answer on the
  // administration port alone, so the section draws none of them.
  it('draws the aliases and none of the installation keys', async () => {
    const api = stubApi()
    mount(api)

    await screen.findByText('default')
    expect(screen.queryByText('Provider keys')).toBeNull()
    expect(screen.queryByText('Anthropic')).toBeNull()
    expect(
      api.GET.mock.calls.some(
        (call: unknown[]) => call[0] === '/api/v1/administration/providers',
      ),
    ).toBe(false)
  })
})
