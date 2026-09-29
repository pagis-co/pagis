// The Plugins settings section: the installed rows with their
// counts, the update chip, the account binding, Update and Uninstall,
// and the Install card that fetches a package, binds an account and
// asks the user to accept the tools that never ask.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, PluginDto } from '../api/client'
import { Plugins } from './Plugins'

function flights(overrides: Partial<PluginDto> = {}): PluginDto {
  return {
    id: 'pl-1',
    name: 'flights',
    state: 'enabled',
    source_kind: 'git',
    source_url: 'https://github.com/acme/flights',
    source_ref: null,
    installed_commit: 'abc1234',
    manifest_version: '2.1',
    created_at: 1,
    updated_at: 1,
    servers: [
      {
        name: 'acme',
        transport: 'stdio',
        command: './bin/acme',
        args: [],
        cwd: null,
        url: null,
        env: [],
        headers: [],
        running: false,
      },
    ],
    fields: [
      {
        name: 'calendar',
        kind: 'connection',
        title: 'Google account',
        description: '',
        required: true,
        provider: 'google',
        capabilities: ['calendar_read'],
      },
    ],
    bindings: [
      {
        field: 'calendar',
        kind: 'connection',
        connection_id: 'conn-1',
        capabilities: ['calendar_read'],
      },
    ],
    tools: [
      { tool: 'search', effect: 'free' },
      { tool: 'book', effect: 'purchase' },
    ],
    frozen_tools: [
      { name: 'flights__search', server: 'acme', description: '', effect: 'free' },
      { name: 'flights__book', server: 'acme', description: '', effect: 'purchase' },
      { name: 'flights__seat', server: 'acme', description: '', effect: 'host' },
      { name: 'flights__cancel', server: 'acme', description: '', effect: 'destructive' },
    ],
    tools_changed: false,
    skills: [{ name: 'plan', description: 'Plan a trip.' }],
    changed: [],
    ...overrides,
  }
}

function jokes(): PluginDto {
  return flights({
    id: 'pl-2',
    name: 'jokes',
    manifest_version: '0.3',
    servers: [],
    fields: [],
    bindings: [],
    tools: [
      { tool: 'tell', effect: 'free' },
      { tool: 'rate', effect: 'free' },
    ],
    frozen_tools: [],
    skills: [
      { name: 'pun', description: '' },
      { name: 'limerick', description: '' },
      { name: 'knock', description: '' },
    ],
  })
}

const connection = {
  id: 'conn-1',
  provider: 'google',
  alias: 'work',
  display_name: 'Work Google',
  status: 'connected',
  auth_mode: 'byo',
  account: 'ada@gmail.com',
  authorized_capabilities: ['calendar_read'],
  created_at: 1,
}

interface Stubs {
  plugins?: PluginDto[]
  fetched?: PluginDto
}

function stubApi(stubs: Stubs = {}) {
  const plugins = stubs.plugins ?? [flights(), jokes()]
  return {
    GET: vi.fn(async (path: string, init?: { params?: { path?: { plugin_id?: string } } }) => {
      switch (path) {
        case '/api/v1/plugins':
          return { data: { items: plugins } }
        case '/api/v1/plugins/{plugin_id}': {
          const id = init?.params?.path?.plugin_id
          return { data: plugins.find((plugin) => plugin.id === id) ?? stubs.fetched }
        }
        case '/api/v1/settings/connections':
          return { data: { items: [connection] } }
        default:
          throw new Error(`unexpected GET ${path}`)
      }
    }),
    POST: vi.fn(async (path: string) => {
      if (path === '/api/v1/plugins') return { data: stubs.fetched ?? flights() }
      return { data: plugins[0] }
    }),
    PUT: vi.fn(async () => ({ data: stubs.fetched ?? flights() })),
    DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <Plugins api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

describe('the installed rows', () => {
  it('count what each plugin ships and say what it is bound to', async () => {
    mount(stubApi())

    expect(await screen.findByText('flights')).toBeTruthy()
    expect(
      await screen.findByText('v2.1 · Servers 1 · Tools 4 · Skills 1'),
    ).toBeTruthy()
    expect(screen.getByText('Bound to Google · ada@gmail.com')).toBeTruthy()

    expect(screen.getByText('jokes')).toBeTruthy()
    expect(screen.getByText('v0.3 · no server · Tools 2 · Skills 3')).toBeTruthy()
    expect(screen.getByText('Needs nothing from you')).toBeTruthy()
    expect(screen.getAllByText('Up to date')).toHaveLength(2)
  })

  it('show the update chip when the tools changed, and Update reads the source again', async () => {
    const api = stubApi({ plugins: [flights({ tools_changed: true })] })
    mount(api)

    expect(await screen.findByText('Update available')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Update flights' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/plugins/{plugin_id}/update', {
        params: { path: { plugin_id: 'pl-1' } },
        body: { artifact_id: undefined },
      }),
    )
  })

  it('name what a disabled plugin still waits for', async () => {
    mount(stubApi({ plugins: [flights({ state: 'disabled', bindings: [] })] }))

    expect(await screen.findByText('Needs Google account')).toBeTruthy()
    expect(screen.getByText('Disabled')).toBeTruthy()
  })

  it('uninstall only after the user confirms', async () => {
    const api = stubApi({ plugins: [jokes()] })
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Uninstall jokes' }))
    expect(api.DELETE).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall jokes for good' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/plugins/{plugin_id}', {
        params: { path: { plugin_id: 'pl-2' } },
      }),
    )
  })

  it('say so when no plugin is installed', async () => {
    mount(stubApi({ plugins: [] }))

    expect(await screen.findByText('No plugin is installed.')).toBeTruthy()
  })
})

describe('the Install card', () => {
  const fetched = flights({ id: 'pl-9', manifest_version: '2.2', bindings: [] })

  async function fetchThePackage(api: ReturnType<typeof stubApi>) {
    mount(api)
    fireEvent.click(await screen.findByRole('button', { name: 'Install a plugin' }))
    fireEvent.change(screen.getByLabelText('Package name or git URL'), {
      target: { value: 'https://github.com/acme/flights' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Fetch' }))
    expect(await screen.findByText('flights 2.2')).toBeTruthy()
  }

  it('fetches the package and sums up what it ships and binds', async () => {
    const api = stubApi({ plugins: [], fetched })
    await fetchThePackage(api)

    expect(api.POST).toHaveBeenCalledWith('/api/v1/plugins', {
      body: {
        source: { kind: 'git', url: 'https://github.com/acme/flights' },
      },
    })
    expect(
      screen.getByText('1 server · 4 tools · 1 skill · binds a Google account'),
    ).toBeTruthy()
  })

  it('binds the chosen account and accepts the tools that never ask', async () => {
    const api = stubApi({ plugins: [], fetched })
    await fetchThePackage(api)

    const install = screen.getByRole('button', { name: 'Install' })
    expect(install.hasAttribute('disabled')).toBe(true)

    await userEvent.click(screen.getByRole('combobox', { name: 'Google account' }))
    await userEvent.click(await screen.findByRole('option', { name: 'Work Google' }))
    expect(install.hasAttribute('disabled')).toBe(true)

    fireEvent.click(screen.getByLabelText('Accept the tools that never ask you'))
    expect(install.hasAttribute('disabled')).toBe(false)
    fireEvent.click(install)

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith(
        '/api/v1/plugins/{plugin_id}/bindings/{field}',
        {
          params: { path: { plugin_id: 'pl-9', field: 'calendar' } },
          body: { kind: 'connection', connection_id: 'conn-1' },
        },
      ),
    )
    await waitFor(() => expect(screen.queryByText('flights 2.2')).toBeNull())
  })

  it('removes a fetched package again on Cancel', async () => {
    const api = stubApi({ plugins: [], fetched })
    await fetchThePackage(api)

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/plugins/{plugin_id}', {
        params: { path: { plugin_id: 'pl-9' } },
      }),
    )
    await waitFor(() => expect(screen.queryByText('flights 2.2')).toBeNull())
  })
})
