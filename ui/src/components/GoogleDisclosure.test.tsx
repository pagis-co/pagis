import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { Browser } from '@capacitor/browser'
import { Capacitor } from '@capacitor/core'
import type { ApiClient, ConnectionDto, ProviderEntryDto } from '../api/client'
import { ConnectGoogle, Connections } from './Connections'
import { ConnectionsPhone } from './phone/ConnectionsPhone'

const entry: ProviderEntryDto = {
  id: 'google', label: 'Google account', blurb: 'Sign in at Google and allow Pagis.',
  kind: 'oauth', fields: [], capabilities: ['mail', 'calendar'], absent_capabilities: [],
  max_instances: null, default_display_name: 'Google', default_alias: '', portal: null,
  set_up: true, browser_sign_in: false,
}
const connection: ConnectionDto = {
  id: 'google-1', provider: 'google', capabilities: ['mail', 'calendar'],
  absent_capabilities: [], installation: false, alias: 'work', display_name: 'Work Google',
  status: 'disconnected', account: null, authorized_capabilities: [], created_at: 1,
}
const copy = [
  'Pagis reads the mail and the calendar that you allow. It copies them into your Workspace on this server.',
  'Your sprites read them when they work for you. They send what they read to the model provider of your sprite.',
  'Pagis does not sell the data. It does not use it for ads.',
  'You can delete it at any time.',
]
const help = 'https://docs.pagis.co/google-data'

vi.mock('@capacitor/browser', () => ({ Browser: { open: vi.fn(), close: vi.fn() } }))

function stub(items: ConnectionDto[] = []) {
  return {
    GET: vi.fn(async (path: string) => ({ data: { items:
      path === '/api/v1/settings/connections/providers' ? [entry] : [...items],
    } })),
    POST: vi.fn(async (path: string): Promise<{ data?: unknown; error?: unknown }> => ({ data:
      path === '/api/v1/settings/connections' ? connection : {
        connection: { ...connection, status: 'connecting' },
        authorization_url: 'https://google.example/authorize',
      },
    })),
    DELETE: vi.fn(async () => ({ response: { ok: true } })),
  }
}

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.clearAllMocks() })

describe.each(['desk', 'phone'] as const)('Google disclosure on the %s', (layout) => {
  function mount(api: ReturnType<typeof stub>, list = false) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const typed = api as unknown as ApiClient
    render(<QueryClientProvider client={client}>
      {layout === 'phone' ? <ConnectionsPhone api={typed} onOpen={() => {}} /> :
        list ? <Connections api={typed} onOpen={() => {}} /> :
          <ConnectGoogle api={typed} entry={entry} onDone={() => {}} />}
    </QueryClientProvider>)
  }

  it('shows the disclosure before authorization and keeps the names when Back is selected', async () => {
    const api = stub()
    const open = vi.fn()
    vi.stubGlobal('open', open)
    mount(api)
    if (layout === 'phone') {
      fireEvent.click(screen.getByRole('button', { name: 'Add a connection' }))
      await screen.findByRole('dialog', { name: 'Add a connection' })
    }
    const name = await screen.findByLabelText('Connection name')
    fireEvent.change(name, { target: { value: 'Work Google' } })
    fireEvent.change(screen.getByLabelText('Short name'), { target: { value: 'work' } })
    expect(screen.queryByRole('button', { name: 'Continue to Google' })).toBeNull()
    expect(api.POST).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    for (const line of copy) expect(screen.getByText(line)).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Delete the Google data that Pagis holds' }).getAttribute('href')).toBe(help)
    expect(api.POST).not.toHaveBeenCalled()
    expect(open).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    expect((screen.getByLabelText('Connection name') as HTMLInputElement).value).toBe('Work Google')
    expect((screen.getByLabelText('Short name') as HTMLInputElement).value).toBe('work')
    expect(screen.queryByRole('button', { name: 'Continue to Google' })).toBeNull()
    expect(api.POST).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    fireEvent.click(screen.getByRole('button', { name: 'Continue to Google' }))
    await waitFor(() => expect(open).toHaveBeenCalledWith('https://google.example/authorize', '_blank', 'noopener'))
    expect(api.POST.mock.calls.map(([path]) => path)).toEqual([
      '/api/v1/settings/connections', '/api/v1/settings/connections/{connection_id}/authorize',
    ])
    expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/connections', {
      body: { provider: 'google', alias: 'work', display_name: 'Work Google', fields: {} },
    })
    expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/connections/{connection_id}/authorize', {
      params: { path: { connection_id: 'google-1' } },
      body: { capabilities: ['gmail_read', 'calendar_read'] },
    })
  })

  it('links each Google Connection row to the deletion guide', async () => {
    const items = [connection, { ...connection, id: 'google-2', alias: 'home', display_name: 'Home Google' }]
    mount(stub(items), true)
    const links = await screen.findAllByRole('link', { name: 'Delete the Google data that Pagis holds' })
    expect(links).toHaveLength(2)
    for (const [index, link] of links.entries()) {
      expect(link.getAttribute('href')).toBe(help)
      expect(link.closest('button')).toBeNull()
      expect(link.closest('.ui-row')?.textContent).toContain(items[index].display_name)
    }
  })

  it('returns to the names after refused authorization and uses edited names on retry', async () => {
    const api = stub()
    const post = api.POST.getMockImplementation()!
    api.POST.mockImplementation(async (path) => path.endsWith('/authorize')
      ? { error: { error: { message: 'Google said no.' } } }
      : post(path))
    mount(api)
    if (layout === 'phone') fireEvent.click(screen.getByRole('button', { name: 'Add a connection' }))
    fireEvent.change(await screen.findByLabelText('Short name'), { target: { value: 'work' } })
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    fireEvent.click(screen.getByRole('button', { name: 'Continue to Google' }))
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('Google said no.'))
    expect((screen.getByLabelText('Short name') as HTMLInputElement).value).toBe('work')
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/connections/{connection_id}', {
      params: { path: { connection_id: 'google-1' } },
    })
    api.POST.mockImplementation(post)
    vi.stubGlobal('open', vi.fn())
    fireEvent.change(screen.getByLabelText('Short name'), { target: { value: 'home' } })
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    fireEvent.click(screen.getByRole('button', { name: 'Continue to Google' }))
    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/connections', {
      body: { provider: 'google', alias: 'home', display_name: 'Google', fields: {} },
    }))
  })

  if (layout === 'phone') it('waits for Continue to Google in the Mobile App and closes the sign-in sheet after connection', async () => {
    vi.spyOn(Capacitor, 'isNativePlatform').mockReturnValue(true)
    const items: ConnectionDto[] = []
    const api = stub(items)
    const open = vi.fn()
    vi.stubGlobal('open', open)
    mount(api)
    fireEvent.click(screen.getByRole('button', { name: 'Add a connection' }))
    fireEvent.change(await screen.findByLabelText('Short name'), { target: { value: 'work' } })
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }))
    expect(api.POST).not.toHaveBeenCalled()
    expect(Browser.open).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Continue to Google' }))
    await waitFor(() => expect(Browser.open).toHaveBeenCalledWith({ url: 'https://google.example/authorize' }))
    expect(open).not.toHaveBeenCalled()
    items.push({ ...connection, status: 'connected' })
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull(), { timeout: 3000 })
    expect(Browser.close).toHaveBeenCalled()
  })
})
