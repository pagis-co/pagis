// The Connection list and the connect flow (ADR-0022): the four
// steps, the waiting state that says where the browser went, what a
// refused exchange leaves on screen, and reauthorizing by capability.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, ConnectionDto, ProviderEntryDto } from '../api/client'
import { Connections } from './Connections'

/** The Provider Catalog the daemon serves, as the picker reads
 *  it. The client holds no list of its own, so the test supplies one.
 *  A person picks only what a person connects: the carrier account and
 *  the mail domain are the installation's. */
const PROVIDERS: ProviderEntryDto[] = [
  {
    id: 'google',
    label: 'Google account',
    blurb: 'Pagis signs in with an OAuth client you own.',
    kind: 'oauth',
    // The local shape of the entry: the person supplies their own
    // Desktop client. A brokered installation serves the account alone.
    fields: [
      {
        key: 'account',
        label: 'Google account',
        hint: 'Google account, e.g. alice@example.com',
        kind: 'text',
        secret: false,
        default: null,
      },
      {
        key: 'client_id',
        label: 'Client ID',
        hint: 'Client ID',
        kind: 'text',
        secret: false,
        default: null,
      },
      {
        key: 'client_secret',
        label: 'Client secret',
        hint: 'Client secret',
        kind: 'text',
        secret: true,
        default: null,
      },
    ],
    capabilities: ['mail', 'calendar'],
    absent_capabilities: [],
    max_instances: null,
    default_display_name: 'Google',
    default_alias: '',
    portal: 'the Google Cloud console',
    // A local entry has no start route, so no browser step asks for a
    // sign-in to Pagis.
    browser_sign_in: false,
  },
]

/** What each provider gives, as the daemon states it on a Connection. */
const CAPABILITIES: Record<string, string[]> = {
  google: ['mail', 'calendar'],
  telnyx: ['telephony', 'texting'],
  plivo: ['telephony'],
  migadu: ['mailboxes'],
  manual: ['mailboxes'],
}

function connected(overrides: Partial<ConnectionDto> = {}): ConnectionDto {
  const provider = overrides.provider ?? 'google'
  return {
    id: 'conn-1',
    provider,
    capabilities: CAPABILITIES[provider] ?? [],
    // Plivo never returns the body of an inbound text (ADR-0020).
    absent_capabilities: provider === 'plivo' ? ['texting'] : [],
    installation: provider !== 'google',
    alias: 'work',
    display_name: 'Work Google',
    status: 'connected',
    auth_mode: 'byo',
    account: 'alice@example.com',
    authorized_capabilities: ['gmail_read', 'calendar_read'],
    created_at: 1,
    ...overrides,
  }
}

type PostResult = { data?: unknown; error?: unknown }

function stubApi(
  items: ConnectionDto[],
  post: (path: string) => Promise<PostResult> = async () => ({ data: connected() }),
  agents: unknown[] = [],
  grants: unknown[] = [],
  numbers: unknown[] = [],
) {
  return {
    GET: vi.fn(async (path: string): Promise<{ data: unknown }> => ({
      data: {
        items:
          path === '/api/v1/settings/connections'
            ? items
            : path === '/api/v1/settings/connections/providers'
              ? PROVIDERS
              : path === '/api/v1/agents'
                ? agents
                : path === '/api/v1/settings/phone-numbers'
                  ? numbers
                  : grants,
        carrier: null,
      },
    })),
    POST: vi.fn(post),
    DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
  }
}

// Every stub here is cast to the client, so the shape is the test's
// own business.
function mount(api: unknown, onOpen: (connectionId: string) => void = () => {}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <Connections api={api as unknown as ApiClient} onOpen={onOpen} />
    </QueryClientProvider>,
  )
}

async function openTheClientStep() {
  fireEvent.click(await screen.findByText('Add a connection'))
  fireEvent.click(screen.getByText('Google account'))
}

function fillTheClientStep() {
  fireEvent.change(screen.getByLabelText('Connection name'), {
    target: { value: 'Work Google' },
  })
  fireEvent.change(screen.getByLabelText('Short name'), {
    target: { value: 'work' },
  })
  fireEvent.change(screen.getByLabelText('Google account'), {
    target: { value: 'alice@example.com' },
  })
  fireEvent.change(screen.getByLabelText('Client ID'), { target: { value: '1234.apps' } })
  fireEvent.change(screen.getByLabelText('Client secret'), {
    target: { value: 'GOCSPX-secret' },
  })
}

describe('Connections', () => {
  it('shows a load failure and lets the user retry', async () => {
    let unavailable = true
    const api = stubApi([])
    const read = api.GET.getMockImplementation()!
    api.GET.mockImplementation(async (path: string) => {
      if (path === '/api/v1/settings/connections' && unavailable) {
        throw new Error('Unavailable')
      }
      return read(path)
    })
    mount(api)
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('Could not load connections'))
    expect(screen.queryByText('No connections.')).toBeNull()
    unavailable = false
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(await screen.findByText('No connections.')).toBeTruthy()
  })

  it('shows a row with the account, the alias and the state as a pill', async () => {
    mount(stubApi([connected()]))

    expect(await screen.findByText('Work Google')).toBeTruthy()
    expect(screen.getByText(/alice@example\.com · known as work/)).toBeTruthy()
    expect(screen.getByText('Connected').className).toContain('ui-badge')
  })

  it('opens the connection page from its card', async () => {
    const onOpen = vi.fn()
    mount(stubApi([connected()]), onOpen)

    fireEvent.click(await screen.findByRole('button', { name: 'Open Work Google' }))
    expect(onOpen).toHaveBeenCalledWith('conn-1')
  })

  it('names a record Google no longer accepts as one that needs the user', async () => {
    mount(stubApi([connected({ status: 'reauth_required' })]))

    expect(await screen.findByText('Reconnect required')).toBeTruthy()
  })

  it('reconnects an expired Google account with its existing access', async () => {
    const api = stubApi([
      connected({
        status: 'reauth_required',
        authorized_capabilities: ['gmail_read', 'gmail_send'],
      }),
    ])
    mount(api)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Reconnect Work Google' }),
    )

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/connections/{connection_id}/authorize',
        {
          params: { path: { connection_id: 'conn-1' } },
          body: { capabilities: ['gmail_read', 'gmail_send'] },
        },
      ),
    )
    expect(screen.queryByLabelText('Send email')).toBeNull()
  })

  it('refreshes the status when Google rejects a reconnect', async () => {
    const api = stubApi(
      [connected({ status: 'reauth_required' })],
      async () => ({
        error: { error: { code: 'validation', message: 'Google said no.' } },
      }),
    )
    mount(api)
    await screen.findByText('Reconnect required')
    const readsBeforeReconnect = api.GET.mock.calls.length

    fireEvent.click(
      screen.getByRole('button', { name: 'Reconnect Work Google' }),
    )

    expect(await screen.findByRole('alert')).toHaveProperty(
      'textContent',
      expect.stringContaining('Google said no.'),
    )
    await waitFor(() =>
      expect(api.GET.mock.calls.length).toBeGreaterThan(readsBeforeReconnect),
    )
  })

  it('connects an account in four steps and says where the browser went', async () => {
    // The exchange stays open until the test resolves it, so the
    // waiting step is observable.
    let finishExchange = () => {}
    const api = stubApi([], async (path) => {
      if (path === '/api/v1/settings/connections') {
        return { data: connected({ status: 'disconnected' }) }
      }
      await new Promise<void>((resolve) => {
        finishExchange = resolve
      })
      return { data: connected() }
    })
    mount(api)

    await openTheClientStep()
    expect(screen.getByText('Your Google client')).toBeTruthy()
    fillTheClientStep()
    fireEvent.click(screen.getByText('Continue at Google'))

    // The third step is explicit: the browser left, this page waits,
    // and the daemon listens on the loopback.
    expect(await screen.findByText('Finish at Google')).toBeTruthy()
    expect(screen.getByText(/left for Google/)).toBeTruthy()
    expect(screen.getByText(/127\.0\.0\.1/)).toBeTruthy()

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/connections', {
        body: {
          provider: 'google',
          alias: 'work',
          display_name: 'Work Google',
          fields: {
            account: 'alice@example.com',
            client_id: '1234.apps',
            client_secret: 'GOCSPX-secret',
          },
        },
      }),
    )
    // A new connection starts read-only.
    expect(api.POST).toHaveBeenCalledWith(
      '/api/v1/settings/connections/{connection_id}/authorize',
      {
        params: { path: { connection_id: 'conn-1' } },
        body: { capabilities: ['gmail_read', 'calendar_read'] },
      },
    )

    finishExchange()
    expect(await screen.findByText('Work Google is connected')).toBeTruthy()
  })

  it('a refused exchange returns to the second step with the values kept', async () => {
    const api = stubApi([], async (path) =>
      path === '/api/v1/settings/connections'
        ? { data: connected({ status: 'disconnected' }) }
        : { error: { error: { code: 'validation', message: 'Google said no.' } } },
    )
    mount(api)

    await openTheClientStep()
    fillTheClientStep()
    fireEvent.click(screen.getByText('Continue at Google'))

    expect(await screen.findByText('Google said no.')).toBeTruthy()
    expect(screen.getByText('Your Google client')).toBeTruthy()
    expect((screen.getByLabelText('Short name') as HTMLInputElement).value).toBe(
      'work',
    )
    expect((screen.getByLabelText('Google account') as HTMLInputElement).value).toBe(
      'alice@example.com',
    )
    // The record that cannot be authorized frees its name for the retry.
    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith(
        '/api/v1/settings/connections/{connection_id}',
        { params: { path: { connection_id: 'conn-1' } } },
      ),
    )
  })

  it('a duplicate name is refused in the words the daemon used', async () => {
    const api = stubApi([], async () => ({
      error: { error: { code: 'conflict', message: 'alias work is taken' } },
    }))
    mount(api)

    await openTheClientStep()
    fillTheClientStep()
    fireEvent.click(screen.getByText('Continue at Google'))

    expect(await screen.findByText('alias work is taken')).toBeTruthy()
    expect(screen.getByText('Your Google client')).toBeTruthy()
  })

  it('widens scopes by reauthorizing for the capabilities the user grants', async () => {
    const api = stubApi([connected()])
    mount(api)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Change access for Work Google' }),
    )
    fireEvent.click(screen.getByLabelText('Send email'))
    fireEvent.click(screen.getByRole('button', { name: 'Authorize Work Google' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/connections/{connection_id}/authorize',
        {
          params: { path: { connection_id: 'conn-1' } },
          body: { capabilities: ['gmail_read', 'calendar_read', 'gmail_send'] },
        },
      ),
    )
  })

  it('deletes a connection from its card', async () => {
    const api = stubApi([connected()])
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Delete Work Google' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith(
        '/api/v1/settings/connections/{connection_id}',
        { params: { path: { connection_id: 'conn-1' } } },
      ),
    )
  })

  // The carrier account is the installation's: the picker offers none,
  // and the card changes nothing of it.
  it('offers no carrier and no mail domain in the picker', async () => {
    mount(stubApi([]))

    fireEvent.click(await screen.findByText('Add a connection'))
    expect(await screen.findByRole('button', { name: 'Google account' })).toBeTruthy()
    for (const name of ['Telnyx', 'Twilio', 'Plivo', 'Migadu', 'Manual mail host']) {
      expect(screen.queryByRole('button', { name })).toBeNull()
    }
  })

  it('disables a provider the workspace holds as many of as it allows', async () => {
    const api = stubApi([connected()])
    const read = api.GET.getMockImplementation()!
    api.GET.mockImplementation(async (path: string) =>
      path === '/api/v1/settings/connections/providers'
        ? { data: { items: [{ ...PROVIDERS[0], max_instances: 1 }] } }
        : read(path),
    )
    mount(api)

    fireEvent.click(await screen.findByText('Add a connection'))
    const google = (await screen.findByRole('button', {
      name: 'Google account',
    })) as HTMLButtonElement
    expect(google.disabled).toBe(true)
    expect(screen.getByText(/already has one/)).toBeTruthy()
  })

  it('shows the carrier with no key, no sign-in and no delete', async () => {
    mount(
      stubApi([
        connected({
          id: 'conn-2',
          provider: 'telnyx',
          alias: 'carrier',
          display_name: 'Telnyx',
          account: null,
          authorized_capabilities: [],
        }),
      ]),
    )

    expect(await screen.findByText('Telnyx')).toBeTruthy()
    expect(screen.getByText(/Telephony carrier/)).toBeTruthy()
    expect(screen.getByText(/An administrator sets it up in the Administration Interface/)).toBeTruthy()
    expect(screen.queryByLabelText('Reauthorize Telnyx')).toBeNull()
    expect(screen.queryByLabelText('Replace the API key for Telnyx')).toBeNull()
    expect(screen.queryByLabelText('Enter the carrier sign-in for Telnyx')).toBeNull()
    expect(screen.queryByLabelText('Delete Telnyx')).toBeNull()
  })

  it("links an administrator to the Administration Interface from the carrier", async () => {
    const api = stubApi([
      connected({
        id: 'conn-2',
        provider: 'telnyx',
        alias: 'carrier',
        display_name: 'Telnyx',
        account: null,
        authorized_capabilities: [],
      }),
    ])
    const read = api.GET.getMockImplementation()!
    api.GET.mockImplementation(async (path: string) =>
      path === '/api/v1/user'
        ? {
            data: {
              role: 'administrator',
              administration: { origin: 'http://127.0.0.1:4401', loopback: true },
            },
          }
        : read(path),
    )
    mount(api)

    const link = await screen.findByRole('link', { name: 'Open the Administration Interface' })
    expect(link.getAttribute('href')).toBe('http://127.0.0.1:4401/providers')
  })

  // A carrier that carries no text says so on its card (ADR-0020), so
  // the user reads it before a number is bought.
  it('names the capabilities a carrier does not have', async () => {
    mount(
      stubApi([
        connected({
          id: 'conn-3',
          provider: 'plivo',
          alias: 'carrier',
          display_name: 'Plivo',
          account: null,
          authorized_capabilities: [],
        }),
      ]),
    )

    const absent = await screen.findByTestId('carrier-absent-capabilities')
    expect(absent.textContent).toContain('does not carry texts')
  })

  it('says nothing absent about a carrier that carries texts', async () => {
    mount(
      stubApi([
        connected({
          id: 'conn-2',
          provider: 'telnyx',
          alias: 'carrier',
          display_name: 'Telnyx',
          account: null,
          authorized_capabilities: [],
        }),
      ]),
    )

    expect(await screen.findByText('Telnyx')).toBeTruthy()
    expect(screen.queryByTestId('carrier-absent-capabilities')).toBeNull()
  })

  // The carrier card lists the workspace's numbers, so a spare
  // one has a home besides every Agent's page.
  it("lists the carrier's numbers with their holder and releases a spare one", async () => {
    const carrier = connected({
      id: 'conn-2',
      provider: 'telnyx',
      alias: 'carrier',
      display_name: 'Telnyx',
      account: null,
      authorized_capabilities: [],
    })
    const api = stubApi(
      [carrier],
      async () => ({ data: connected() }),
      [{ id: 'ag1', name: 'Robin', status: 'active' }],
      [],
      [
        { id: 'pn1', e164: '+14155550123', status: 'assigned', agent_id: 'ag1' },
        { id: 'pn2', e164: '+14155550124', status: 'unassigned', agent_id: null },
        { id: 'pn3', e164: '+14155550125', status: 'released', agent_id: null },
      ],
    )
    mount(api)

    expect(await screen.findByText('+1 415 555 0123')).toBeTruthy()
    expect(screen.getByText('Held by Robin')).toBeTruthy()
    expect(screen.getByText('+1 415 555 0124')).toBeTruthy()
    expect(screen.getByText('Unassigned')).toBeTruthy()
    // A released number went back to the carrier and is not listed.
    expect(screen.queryByText('+1 415 555 0125')).toBeNull()
    // Only a spare number can be released here; a held one is released
    // from its Agent's page, which names the Agent that loses the line.
    expect(screen.queryByLabelText('Release +14155550123')).toBeNull()

    fireEvent.click(screen.getByLabelText('Release +14155550124'))
    fireEvent.click(await screen.findByText('Release it'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/phone-numbers/{phone_number_id}/release',
        { params: { path: { phone_number_id: 'pn2' } } },
      ),
    )
  })

  it('adopts a number the account already holds from the carrier card', async () => {
    const api = stubApi([
      connected({
        id: 'conn-2',
        provider: 'telnyx',
        alias: 'carrier',
        display_name: 'Telnyx',
        account: null,
        authorized_capabilities: [],
      }),
    ])
    mount(api)

    expect(await screen.findByText('No numbers yet.')).toBeTruthy()
    fireEvent.click(screen.getByText('Add a number you own'))
    fireEvent.change(screen.getByLabelText('Phone number'), {
      target: { value: '+14155550199' },
    })
    fireEvent.click(screen.getByText('Add number'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/phone-numbers/adopt',
        { body: { e164: '+14155550199', agent_id: undefined } },
      ),
    )
  })
})

// The Mailbox Provider Connections (ADR-0019): the mail domain is the
// installation's, so its row says what the host cannot do and which
// Agents hold a mailbox on it, and changes nothing of the domain.
describe('Connections mailbox providers', () => {
  const capable = {
    domain: 'example.com',
    imap_host: 'imap.migadu.com',
    imap_port: 993,
    smtp_host: 'smtp.migadu.com',
    smtp_port: 465,
    idle: true,
    outgoing_cap: true,
    delete_mailbox: true,
    reset_password: true,
  }

  const migadu = connected({
    id: 'cx1',
    provider: 'migadu',
    alias: 'mail',
    display_name: 'Agent mail',
    account: null,
    authorized_capabilities: [],
    mail: capable,
  } as Partial<ConnectionDto>)

  const manual = connected({
    id: 'cx2',
    provider: 'manual',
    alias: 'host',
    display_name: 'Mail host',
    account: null,
    authorized_capabilities: [],
    mail: {
      ...capable,
      imap_host: 'imap.example.com',
      smtp_host: 'smtp.example.com',
      idle: false,
      outgoing_cap: false,
      delete_mailbox: false,
      reset_password: false,
    },
  } as Partial<ConnectionDto>)

  function mailApi({
    items = [migadu] as ConnectionDto[],
    mailboxes = [] as unknown[],
    agents = [] as unknown[],
    post = (async () => ({ data: migadu })) as (path: string) => Promise<PostResult>,
    del = undefined as (() => Promise<unknown>) | undefined,
  } = {}) {
    return {
      GET: vi.fn(async (path: string) => {
        if (path === '/api/v1/settings/connections') return { data: { items } }
        if (path === '/api/v1/settings/connections/providers') {
          return { data: { items: PROVIDERS } }
        }
        if (path === '/api/v1/settings/mailboxes') {
          return { data: { items: mailboxes } }
        }
        if (path === '/api/v1/agents') return { data: { items: agents } }
        return { data: { items: [] } }
      }),
      POST: vi.fn(post),
      DELETE: vi.fn(
        del ?? (async () => ({ error: undefined, response: { ok: true } })),
      ),
    }
  }

  it('shows the domain, the alias, the status and the endpoints', async () => {
    mount(mailApi())

    const card = await screen.findByTestId('mailbox-provider-card')
    expect(card.textContent).toContain('Mailbox provider')
    expect(card.textContent).toContain('example.com')
    expect(card.textContent).toContain('mail')
    expect(card.textContent).toContain('Connected')
    expect(card.textContent).toContain('imap.migadu.com:993')
    expect(card.textContent).toContain('smtp.migadu.com:465')
    // A host with every capability has nothing absent to report.
    expect(screen.queryByTestId('mailbox-absent-capabilities')).toBeNull()
  })

  it('names the capabilities a manual host does not have', async () => {
    mount(mailApi({ items: [manual] }))

    const absent = await screen.findByTestId('mailbox-absent-capabilities')
    expect(absent.textContent).toContain('wait for new mail')
    expect(absent.textContent).toContain('delete a mailbox')
    expect(absent.textContent).toContain('mint a password')
  })

  it('lists the mailboxes on the provider with the Agent that holds each', async () => {
    mount(
      mailApi({
        mailboxes: [
          {
            id: 'mb1',
            agent_id: 'ag1',
            address: 'ada@example.com',
            state: 'active',
            connection_id: 'cx1',
            outgoing_cap: 20,
            sends_today: 0,
            created_at: 1,
          },
        ],
        agents: [{ id: 'ag1', name: 'Ada', status: 'active' }],
      }),
    )

    expect(await screen.findByText('ada@example.com · Ada')).toBeTruthy()
  })

  it('offers no delete and no key on the mail domain', async () => {
    mount(mailApi({ items: [migadu, manual] }))

    await screen.findAllByTestId('mailbox-provider-card')
    for (const name of ['Agent mail', 'Mail host']) {
      expect(screen.queryByLabelText(`Delete ${name}`)).toBeNull()
      expect(screen.queryByLabelText(`Replace the API key for ${name}`)).toBeNull()
    }
    expect(screen.getAllByText(/example.com is this installation's mail domain/).length).toBe(2)
  })

  it('a Gmail row says what an agent can do with the inbox', async () => {
    mount(mailApi({ items: [connected()] }))

    expect(await screen.findByText('work')).toBeTruthy()
    expect(
      screen.getByText(/can read and send from this inbox/),
    ).toBeTruthy()
    expect(screen.queryByText(/mail__/)).toBeNull()
  })
})

/** The brokered Google flow: the installation holds the OAuth
 *  client, so the form asks for the account alone, the page opens the
 *  start route in a new tab, which goes on to Google, and the card
 *  catches up when the redirect lands. */
describe('Connections brokered Google', () => {
  /** The catalog a brokered installation serves: the account field
   *  alone. `browserSignIn` is false on a Local Installation with
   *  Remote Access off, where the browser needs no Session. */
  function brokeredCatalog(browserSignIn: boolean): ProviderEntryDto[] {
    return PROVIDERS.map((entry) =>
      entry.id === 'google'
        ? {
            ...entry,
            blurb: "Pagis signs in with this installation's own Google client.",
            fields: entry.fields.filter((field) => field.key === 'account'),
            browser_sign_in: browserSignIn,
          }
        : entry,
    )
  }

  const SIGN_IN = /If that tab asks you to sign in to Pagis, sign in as yourself/

  function brokeredApi(
    items: ConnectionDto[],
    authorizationUrl: string | null = 'https://pagis.example.net/api/v1/connections/google/start?state=abc',
    browserSignIn = true,
  ) {
    return {
      GET: vi.fn(async (path: string) => ({
        data: {
          items:
            path === '/api/v1/settings/connections'
              ? [...items]
              : path === '/api/v1/settings/connections/providers'
                ? brokeredCatalog(browserSignIn)
                : [],
          carrier: null,
        },
      })),
      POST: vi.fn(async (path: string) =>
        path === '/api/v1/settings/connections'
          ? {
              data: connected({ status: 'disconnected', auth_mode: 'brokered' }),
            }
          : {
              data: {
                connection: connected({
                  status: 'connecting',
                  auth_mode: 'brokered',
                }),
                authorization_url: authorizationUrl,
              },
            },
      ),
      DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
    }
  }

  it('asks for the account alone and opens the start route in a new tab', async () => {
    const open = vi.fn()
    vi.stubGlobal('open', open)
    const items: ConnectionDto[] = []
    const api = brokeredApi(items)
    mount(api)

    fireEvent.click(await screen.findByText('Add a connection'))
    fireEvent.click(screen.getByText('Google account'))

    expect(screen.getByText('The account to connect')).toBeTruthy()
    expect(
      screen.getByText(/this installation's own Google client/),
    ).toBeTruthy()
    // The person supplies nothing of the client.
    expect(screen.queryByLabelText('Client ID')).toBeNull()
    expect(screen.queryByLabelText('Client secret')).toBeNull()

    fireEvent.change(screen.getByLabelText('Short name'), {
      target: { value: 'work' },
    })
    fireEvent.change(screen.getByLabelText('Google account'), {
      target: { value: 'alice@example.com' },
    })
    fireEvent.click(screen.getByText('Continue at Google'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/connections', {
        body: {
          provider: 'google',
          alias: 'work',
          display_name: 'Google',
          fields: { account: 'alice@example.com' },
        },
      }),
    )
    await waitFor(() =>
      expect(open).toHaveBeenCalledWith(
        'https://pagis.example.net/api/v1/connections/google/start?state=abc',
        '_blank',
        'noopener',
      ),
    )
    // The flow waits for the redirect: it does not claim the account is
    // connected because the request returned.
    expect(await screen.findByText('Finish at Google')).toBeTruthy()
    expect(screen.getByText(/opened in a new tab/)).toBeTruthy()
    // The new tab has no Session of its own on a Server or in Remote
    // Access, so it can ask the person to sign in first.
    expect(screen.getByText(SIGN_IN)).toBeTruthy()

    // Google sends the person back, the callback lands, and the list is
    // what tells the page.
    items.push(connected({ auth_mode: 'brokered' }))
    // The page polls the list while it waits, so the record reaching
    // `connected` is what turns the step over. An open client is also
    // told by the `connection.changed` event the callback publishes.
    expect(
      await screen.findByText('Google is connected', undefined, {
        timeout: 5000,
      }),
    ).toBeTruthy()
    vi.unstubAllGlobals()
  })

  /** A Local Installation with Remote Access off has one Person,
   *  and its start route asks the new tab for no sign-in. The copy does
   *  not mention one. */
  it('mentions no sign-in in the new tab on a single-Person installation', async () => {
    const open = vi.fn()
    vi.stubGlobal('open', open)
    const api = brokeredApi([], undefined, false)
    mount(api)

    fireEvent.click(await screen.findByText('Add a connection'))
    fireEvent.click(screen.getByText('Google account'))
    fireEvent.change(screen.getByLabelText('Short name'), {
      target: { value: 'work' },
    })
    fireEvent.change(screen.getByLabelText('Google account'), {
      target: { value: 'alice@example.com' },
    })
    fireEvent.click(screen.getByText('Continue at Google'))

    expect(await screen.findByText('Finish at Google')).toBeTruthy()
    expect(screen.getByText(/opened in a new tab/)).toBeTruthy()
    expect(screen.queryByText(SIGN_IN)).toBeNull()
    vi.unstubAllGlobals()
  })

  it.each([
    ['names the sign-in on a Server or in Remote Access', true],
    ['names no sign-in on a single-Person installation', false],
  ])('the card that changes access %s', async (_, browserSignIn) => {
    const api = brokeredApi([connected({ auth_mode: 'brokered' })], undefined, browserSignIn)
    mount(api)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Change access for Work Google' }),
    )

    expect(screen.getByText(/Pagis opens Google in a new tab/)).toBeTruthy()
    expect(screen.queryByText(SIGN_IN) !== null).toBe(browserSignIn)
  })

  it('a brokered card that is connecting says to finish at Google', async () => {
    const api = brokeredApi([
      connected({ status: 'connecting', auth_mode: 'brokered' }),
    ])
    mount(api)

    fireEvent.click(await screen.findByText('Work Google'))

    expect(
      await screen.findByText(/Finish signing in at Google in the tab/),
    ).toBeTruthy()
  })
})
