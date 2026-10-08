// Settings in three groups: the section list, the section each
// route opens, the copy pass that keeps the internals out of the text,
// and the actions of the sections.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { useState } from 'react'

import type { ApiClient } from '../api/client'
import {
  SETTINGS_GROUPS,
  SETTINGS_SECTIONS,
  SettingsPanel,
  type SettingsSection,
} from './SettingsPanel'

const CONNECTION = {
  id: 'conn-1',
  provider: 'google',
  capabilities: ['mail', 'calendar'],
  alias: 'work',
  display_name: 'Work Google',
  status: 'connected',
  account: 'alice@example.com',
  authorized_capabilities: ['gmail_read', 'calendar_read'],
  created_at: 1,
}

/** The picker's catalog. A person connects their own accounts only;
 *  the carrier and the mail domain are the installation's. The entry
 *  here is a `fields` one, so the generic form draws it. */
const PROVIDERS = [
  {
    id: 'notes',
    label: 'Notes service',
    blurb: 'Paste an API key from the notes service.',
    kind: 'fields',
    fields: [
      {
        key: 'api_key',
        label: 'Notes API key',
        hint: 'Notes API key',
        kind: 'text',
        secret: true,
        default: null,
      },
    ],
    capabilities: ['notes'],
    absent_capabilities: [],
    max_instances: 1,
    default_display_name: 'Notes',
    default_alias: 'notes',
    portal: null,
  },
]

const ADMINISTRATOR = {
  id: 'user-1',
  name: 'Ada',
  email: null,
  role: 'administrator',
  administration: { origin: 'http://127.0.0.1:4401', loopback: true },
}

const TRUST_LIST = {
  items: [
    {
      id: 'trust-1',
      agent_id: null,
      subject: 'number',
      value: '+14155550123',
      tier: 'owner',
      label: 'Home',
    },
  ],
  own_addresses: [{ address: 'owner@example.com', connection_alias: 'mail' }],
  keypad_code: { configured: true },
}

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
                  candidates: ['anthropic/claude-sonnet-4-6'],
                  settings: [],
                  updated_at: 1,
                },
              ],
            },
          }
        case '/api/v1/grants':
        case '/api/v1/plugins':
        case '/api/v1/agents':
        case '/api/v1/settings/mailboxes':
          return { data: { items: [] } }
        case '/api/v1/settings/phone-numbers':
          return { data: { items: [], carrier: null } }
        case '/api/v1/settings/connections':
          return { data: { items: [CONNECTION] } }
        case '/api/v1/settings/connections/providers':
          return { data: { items: PROVIDERS } }
        case '/api/v1/settings/credentials':
          return {
            data: {
              items: [
                {
                  id: 'cred-1',
                  domain: 'example.com',
                  username: 'alice@example.com',
                  login_url: 'https://example.com/login',
                  provenance: 'user_supplied',
                  has_totp: false,
                  owner_agent_id: null,
                  created_at: 1,
                },
              ],
            },
          }
        case '/api/v1/settings/retention':
          return {
            data: {
              items: [
                { kind: 'screenshot', retain_days: null },
                { kind: 'call_recording', retain_days: 30 },
              ],
            },
          }
        case '/api/v1/settings/trust-list':
          return { data: TRUST_LIST }
        case '/api/v1/user':
          return { data: ADMINISTRATOR }
        default:
          throw new Error(`unexpected GET ${path}`)
      }
    }),
    POST: vi.fn(async () => ({
      data: {
        id: 'conn-2',
        alias: 'fast',
        candidates: ['openai/gpt-5.4-mini'],
        updated_at: 2,
      },
    })),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
  }
}

/** `/settings/:section` holds the open section; this stands in for it. */
function SettingsHarness({
  api,
  start,
  isAdministrator = true,
}: {
  api: ApiClient
  start: SettingsSection
  isAdministrator?: boolean
}) {
  const [section, setSection] = useState<SettingsSection>(start)
  return (
    <SettingsPanel
      api={api}
      section={section}
      onSelectSection={setSection}
      onOpenConnection={() => {}}
      isAdministrator={isAdministrator}
    />
  )
}

function mount(
  api: ReturnType<typeof stubApi>,
  start: SettingsSection = 'connections',
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <SettingsHarness api={api as unknown as ApiClient} start={start} />
    </QueryClientProvider>,
  )
}

/** The words the user never has to learn: a tool name, the protocol a
 *  carrier speaks, and a stock of numbers. */
const INTERNALS = [/__/, /\bSIP\b/i, /\bpool\b/i]

/** Everything the user reads: the text, and the labels and the hints
 *  that stand in a box the text does not fill. */
function renderedText(root: HTMLElement): string {
  const attributes = [...root.querySelectorAll('[aria-label], [placeholder]')]
    .flatMap((element) => [
      element.getAttribute('aria-label') ?? '',
      element.getAttribute('placeholder') ?? '',
    ])
    .join(' ')
  return `${root.textContent ?? ''} ${attributes}`
}

describe('SettingsPanel', () => {
  it('lists the sections in three groups and opens on Connections', async () => {
    mount(stubApi())

    expect(SETTINGS_GROUPS.map((group) => group.label)).toEqual([
      'Access',
      'Models',
      'System',
    ])
    for (const group of SETTINGS_GROUPS) {
      expect(screen.getByRole('group', { name: group.label })).toBeTruthy()
    }
    expect(
      screen.getByRole('button', { name: 'Connections' }).getAttribute('aria-current'),
    ).toBe('page')
    expect(await screen.findByText('Work Google')).toBeTruthy()
  })

  it('opens each section from the list', async () => {
    mount(stubApi())

    for (const section of SETTINGS_SECTIONS) {
      await userEvent.click(screen.getByRole('button', { name: section.label }))
      expect(
        screen.getByRole('button', { name: section.label }).getAttribute('aria-current'),
      ).toBe('page')
      expect(
        screen.getByRole('heading', { name: section.label }),
      ).toBeTruthy()
    }
  })

  it('names no tool, no protocol and no second name in what it renders', async () => {
    for (const section of SETTINGS_SECTIONS) {
      const view = mount(stubApi(), section.value)
      await screen.findByRole('heading', { name: section.label })
      const text = renderedText(view.container as HTMLElement)
      for (const internal of INTERNALS) {
        expect(
          internal.test(text),
          `${section.label} says ${internal} in "${text}"`,
        ).toBe(false)
      }
      view.unmount()
    }
  })

  it('adds a connection', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByText('Add a connection'))
    fireEvent.click(screen.getByText('Notes service'))
    fireEvent.change(screen.getByLabelText('Notes API key'), {
      target: { value: 'KEY0000' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Connect Notes service' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/connections', {
        body: {
          provider: 'notes',
          alias: 'notes',
          display_name: 'Notes',
          fields: { api_key: 'KEY0000' },
        },
      }),
    )
  })

  it('adds a trusted contact from the inline add row', async () => {
    const api = stubApi()
    mount(api, 'trusted-contacts')

    const listed = await screen.findByText(/\+1 415 555 0123/)
    const numbers = screen.getByTestId('trust-list-numbers')
    const control = screen.getByLabelText('New number')
    expect(numbers.contains(listed)).toBe(true)
    expect(numbers.contains(control)).toBe(true)
    // The add row is the last row of the frame.
    expect(
      listed.compareDocumentPosition(control) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()

    fireEvent.change(control, { target: { value: '+14155550188' } })
    fireEvent.click(within(numbers).getByRole('button', { name: 'Add' }))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/trust-list', {
        body: { value: '+14155550188', tier: 'trusted', label: '' },
      }),
    )
  })

  it('lists and deletes connections', async () => {
    const api = stubApi()
    mount(api)

    expect(await screen.findByText('Work Google')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Delete Work Google' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith(
        '/api/v1/settings/connections/{connection_id}',
        { params: { path: { connection_id: 'conn-1' } } },
      ),
    )
  })
})

describe('Administration section', () => {
  // The installation's settings answer on the administration port, so
  // the product draws one link to it and none of them.
  it('links an administrator to the Administration Interface and says how to reach a loopback port', async () => {
    const api = stubApi()
    mount(api, 'administration')

    const link = await screen.findByRole('link', { name: 'Open the Administration Interface' })
    expect(link.getAttribute('href')).toBe('http://127.0.0.1:4401/')
    expect(screen.getByText('ssh -L 4401:127.0.0.1:4401 you@your-server')).toBeTruthy()
    for (const path of ['/api/v1/settings/system', '/api/v1/administration/providers']) {
      expect(api.GET.mock.calls.some((call: unknown[]) => call[0] === path)).toBe(false)
    }
  })

  it('names no tunnel where the port binds a private address', async () => {
    const api = stubApi()
    const read = api.GET.getMockImplementation()!
    api.GET.mockImplementation(async (path: string) =>
      path === '/api/v1/user'
        ? {
            data: {
              ...ADMINISTRATOR,
              administration: { origin: 'http://10.0.0.5:4401', loopback: false },
            },
          }
        : read(path),
    )
    mount(api, 'administration')

    const link = await screen.findByRole('link', { name: 'Open the Administration Interface' })
    expect(link.getAttribute('href')).toBe('http://10.0.0.5:4401/')
    expect(screen.queryByText(/ssh -L/)).toBeNull()
  })
})

describe('Sound section', () => {
  it('sits under System and shows the sound switch, off by default', async () => {
    mount(stubApi())
    const system = SETTINGS_GROUPS.find((group) => group.label === 'System')
    expect(system?.sections.map((section) => section.value)).toContain('sound')
    await userEvent.click(screen.getByRole('button', { name: 'Sound' }))
    expect(screen.getByTestId('sound-settings')).toBeTruthy()
    expect(screen.getByRole('switch').getAttribute('aria-checked')).toBe('false')
  })
})
