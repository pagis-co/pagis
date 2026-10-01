// The Sessions section: the person's own signed-in browsers and apps,
// the removal of one, and the link that signs one more in.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, MySessionDto } from '../../api/client'
import { Sessions, sessionLabel } from './Sessions'

const THIS_ONE: MySessionDto = {
  id: 's-this',
  client_kind: 'browser',
  client_name: 'Firefox on Linux',
  current: true,
  created_at: 1_700_000_000_000,
  last_used_at: 1_700_000_100_000,
  expires_at: 1_702_592_100_000,
}

const PHONE: MySessionDto = {
  id: 's-phone',
  client_kind: 'browser',
  client_name: 'Safari on iOS',
  current: false,
  created_at: 1_700_000_000_000,
  last_used_at: 1_700_000_200_000,
  expires_at: 1_702_592_200_000,
}

const LINK = {
  url: 'https://pagis.example/sign-in#secret',
  expires_at: Date.now() + 5 * 60_000,
  qr_svg: '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
}

function stubApi() {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/settings/sessions') return { data: { items: [THIS_ONE, PHONE] } }
      throw new Error(`unexpected GET ${path}`)
    }),
    POST: vi.fn(async (path: string) => {
      if (path === '/api/v1/settings/sign-in-links') return { data: LINK }
      throw new Error(`unexpected POST ${path}`)
    }),
    DELETE: vi.fn(async () => ({ response: new Response(null, { status: 204 }) })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <Sessions api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

describe('Sessions', () => {
  it('lists each session and says which one is this one', async () => {
    mount(stubApi())

    const rows = await screen.findAllByTestId('session-row')
    expect(rows).toHaveLength(2)
    expect(within(rows[0]).getByText('Firefox on Linux')).toBeTruthy()
    expect(within(rows[0]).getByText('This session')).toBeTruthy()
    // This session signs out from the account menu, not from the list.
    expect(within(rows[0]).queryByRole('button', { name: /Remove/ })).toBeNull()
    expect(within(rows[1]).getByText('Safari on iOS')).toBeTruthy()
    expect(within(rows[1]).queryByText('This session')).toBeNull()
  })

  it('removes another session and reads the list again', async () => {
    const api = stubApi()
    mount(api)

    await userEvent.click(await screen.findByRole('button', { name: 'Remove Safari on iOS' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/sessions/{session_id}', {
        params: { path: { session_id: 's-phone' } },
      }),
    )
    await waitFor(() => expect(api.GET).toHaveBeenCalledTimes(2))
  })

  it('makes a sign-in link and shows it with its QR code', async () => {
    const api = stubApi()
    mount(api)
    await screen.findAllByTestId('session-row')

    await userEvent.click(screen.getByRole('button', { name: 'Sign in another browser or app' }))

    expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/sign-in-links')
    const dialog = await screen.findByRole('dialog')
    expect((within(dialog).getByLabelText('Sign-in link') as HTMLInputElement).value).toBe(
      LINK.url,
    )
    expect(within(dialog).getByAltText('QR code of the sign-in link')).toBeTruthy()
  })

  it('says so when the list cannot be read', async () => {
    const api = stubApi()
    api.GET = vi.fn(async () => ({ error: { message: 'down' } }) as never)
    mount(api)

    expect(await screen.findByText('Your sessions could not be read.')).toBeTruthy()
  })
})

describe('sessionLabel', () => {
  it('names a browser by itself and its system, and a Client App by its machine', () => {
    expect(sessionLabel(PHONE)).toBe('Safari on iOS')
    expect(sessionLabel({ ...PHONE, client_name: null })).toBe('Browser')
    expect(sessionLabel({ ...PHONE, client_kind: 'desktop', client_name: 'Ada’s Mac' })).toBe(
      'Client App on Ada’s Mac',
    )
    expect(sessionLabel({ ...PHONE, client_kind: 'desktop', client_name: null })).toBe(
      'Client App',
    )
  })
})
