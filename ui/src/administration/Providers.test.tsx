// The Providers view of the Administration Interface: the parts each
// provider declares, drawn from the daemon's answer, and the one set of
// routes that sets each part up, tests it and removes it.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, ProviderSetupDto, SetupPartDto } from '../api/client'
import { Providers } from './Providers'

function field(key: string, label: string, secret = false, value: string | null = null) {
  return { key, label, hint: label, kind: 'text', secret, default: value }
}

function part(overrides: Partial<SetupPartDto> & Pick<SetupPartDto, 'id' | 'kind'>): SetupPartDto {
  return {
    label: overrides.id,
    blurb: 'What this part is.',
    fields: [],
    configured: false,
    status: null,
    connection_id: null,
    testable: false,
    facts: [],
    ...overrides,
  }
}

const anthropic: ProviderSetupDto = {
  provider: 'anthropic',
  label: 'Anthropic',
  group: 'models',
  parts: [
    part({
      id: 'key',
      kind: 'model_key',
      label: 'API key',
      fields: [field('api_key', 'API key', true)],
      configured: true,
      facts: [{ label: 'Source', value: 'secrets.enc' }],
    }),
  ],
}

const google: ProviderSetupDto = {
  provider: 'google',
  label: 'Google account',
  group: 'accounts',
  parts: [
    part({
      id: 'oauth-client',
      kind: 'oauth_client',
      label: 'Google OAuth client',
      fields: [field('client_id', 'Client ID'), field('client_secret', 'Client secret', true)],
      facts: [
        {
          label: 'Redirect URI',
          value: 'https://pagis.example.net/api/v1/connections/google/callback',
        },
      ],
    }),
  ],
}

function telnyx(connected: boolean): ProviderSetupDto {
  return {
    provider: 'telnyx',
    label: 'Telnyx',
    group: 'telephony',
    parts: [
      part({
        id: 'connection',
        kind: 'connection',
        label: 'Carrier account',
        fields: [field('api_key', 'Carrier API key', true)],
        configured: connected,
        status: connected ? 'connected' : null,
        connection_id: connected ? 'con-1' : null,
        testable: true,
      }),
      part({
        id: 'sip',
        kind: 'sip_credential',
        label: 'SIP sign-in',
        fields: [
          field('username', 'SIP username'),
          field('password', 'SIP password', true),
          field('domain', 'SIP server', false, 'sip.telnyx.com'),
        ],
      }),
    ],
  }
}

function apiOf(items: ProviderSetupDto[]) {
  return {
    GET: vi.fn(async () => ({ data: { items } })),
    PUT: vi.fn(async () => ({ data: items[0] })),
    POST: vi.fn(async () => ({ data: items[0] })),
    DELETE: vi.fn(async () => ({ data: items[0] })),
  }
}

function mount(api: ReturnType<typeof apiOf>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <Providers api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

describe('Providers', () => {
  it('groups every provider the daemon declares and states each part', async () => {
    const api = apiOf([anthropic, google, telnyx(false)])
    mount(api)

    expect(await screen.findByText('Model providers')).toBeTruthy()
    expect(screen.getByText('Accounts')).toBeTruthy()
    expect(screen.getByText('Phone carriers')).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith('/api/v1/administration/providers')
    // The redirect URI is what the administrator pastes into Google.
    expect(
      screen.getByText('https://pagis.example.net/api/v1/connections/google/callback'),
    ).toBeTruthy()
    // A SIP sign-in waits for the carrier account it signs in to.
    expect(screen.getByText('Set up the Carrier account first.')).toBeTruthy()
    expect(screen.queryByLabelText('Set up the Telnyx SIP sign-in')).toBeNull()
  })

  it('sets up a part with the fields the daemon declares', async () => {
    const api = apiOf([google])
    mount(api)

    fireEvent.click(await screen.findByLabelText('Set up the Google account Google OAuth client'))
    fireEvent.change(screen.getByLabelText('Google account Client ID'), {
      target: { value: ' web.apps.googleusercontent.com ' },
    })
    fireEvent.change(screen.getByLabelText('Google account Client secret'), {
      target: { value: 'GOCSPX-installation' },
    })
    fireEvent.click(screen.getByText('Save'))

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/administration/providers/{provider}/{part}', {
        params: { path: { provider: 'google', part: 'oauth-client' } },
        body: {
          fields: {
            client_id: 'web.apps.googleusercontent.com',
            client_secret: 'GOCSPX-installation',
          },
        },
      }),
    )
  })

  it('asks an existing carrier account for a new key alone, and tests and removes it', async () => {
    const api = apiOf([telnyx(true)])
    mount(api)

    fireEvent.click(await screen.findByLabelText('Replace the Telnyx Carrier account'))
    const form = screen.getByLabelText('Set up Telnyx Carrier account')
    expect(within(form).queryAllByRole('textbox').length).toBe(0)
    expect(within(form).getByLabelText('Telnyx Carrier API key')).toBeTruthy()

    fireEvent.click(screen.getByLabelText('Test the Telnyx Carrier account'))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/administration/providers/{provider}/{part}/test',
        { params: { path: { provider: 'telnyx', part: 'connection' } } },
      ),
    )

    fireEvent.click(screen.getByLabelText('Remove the Telnyx Carrier account'))
    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith(
        '/api/v1/administration/providers/{provider}/{part}',
        { params: { path: { provider: 'telnyx', part: 'connection' } } },
      ),
    )
    // The SIP sign-in is open once the account exists, with the
    // carrier's own server filled in.
    fireEvent.click(screen.getByLabelText('Set up the Telnyx SIP sign-in'))
    expect((screen.getByLabelText('Telnyx SIP server') as HTMLInputElement).value).toBe(
      'sip.telnyx.com',
    )
  })

  it('offers no test for a part that proves itself when it is used', async () => {
    mount(apiOf([anthropic]))

    expect(await screen.findByLabelText('Remove the Anthropic API key')).toBeTruthy()
    expect(screen.queryByLabelText('Test the Anthropic API key')).toBeNull()
  })

  it('shows the refusal the daemon answers', async () => {
    const api = apiOf([telnyx(true)])
    api.DELETE.mockResolvedValueOnce({
      error: {
        error: {
          code: 'conflict',
          message: 'this carrier still carries a phone number. Release the numbers first.',
        },
      },
    } as never)
    mount(api)

    fireEvent.click(await screen.findByLabelText('Remove the Telnyx Carrier account'))

    expect((await screen.findByRole('alert')).textContent).toContain('Release the numbers first')
  })
})
