import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ReactNode } from 'react'

import type { ApiClient, ChannelDto } from './api/client'
import { useChannels } from './queries'
import { CurrentWorkspaceProvider, privateWorkspaceKey } from './workspace'
import { WidgetBlock } from './blocks/WidgetBlock'
import { PageView } from './components/memory/PageView'

vi.mock('./api/client', async () => ({
  ...await vi.importActual<typeof import('./api/client')>('./api/client'),
  fetchWidgetPage: vi.fn(async () => '<p>Weather</p>'),
}))
vi.mock('./blocks/widget', async () => ({
  ...await vi.importActual<typeof import('./blocks/widget')>('./blocks/widget'),
  sandboxOrigin: () => 'http://127.0.0.1:4400',
}))

const workspace = (id: string) => ({
  id,
  name: id,
  timezone: 'UTC',
  chief_of_staff_agent_id: null,
})
const otherChannel: ChannelDto = {
  id: 'channel-1',
  workspace_id: 'other-workspace',
  kind: 'dm',
  title: 'Other',
  agent_ids: [],
  user_member: true,
  created_at: 1,
  updated_at: 1,
}

function mount(ui: (api: ApiClient) => ReactNode, channels: ChannelDto[] = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const api = {
    GET: vi.fn(async (path: string): Promise<{ data: unknown }> => {
      if (path === '/api/v1/workspace') return { data: workspace('workspace-A') }
      if (path === '/api/v1/channels') return { data: { items: channels } }
      if (path === '/api/v1/memory/file') return { data: {
        title: 'Person', content: '# Person\n\n## Facts\n\n## Schedules\n\n---\n\n## Timeline\n\n### Entry\n\nSource reference: `gmail:connection-1:item-1:1`\n\nSource time: 1\n\n> A source.\n',
      } }
      return { data: {} }
    }),
    POST: vi.fn(async () => ({ data: {} })),
  }
  const typedApi = api as unknown as ApiClient
  render(
    <QueryClientProvider client={client}>
      <CurrentWorkspaceProvider api={typedApi}>
        {ui(typedApi)}
      </CurrentWorkspaceProvider>
    </QueryClientProvider>,
  )
  return { client, api }
}

describe('the current Workspace', () => {
  it('reads again when the Workspace changes and keeps each result separate', async () => {
    function Channels({ api }: { api: ApiClient }) {
      const channels = useChannels(api)
      return <p>{channels.data?.[0]?.title}</p>
    }
    const { client, api } = mount(api => <Channels api={api} />, [{ ...otherChannel, title: 'A channel' }])
    await screen.findByText('A channel')
    expect(api.GET.mock.calls.filter(([path]) => path === '/api/v1/workspace')).toHaveLength(1)
    api.GET.mockImplementation(async (path) => path === '/api/v1/channels'
      ? { data: { items: [{ ...otherChannel, title: 'B channel' }] } }
      : { data: workspace('workspace-B') })
    act(() => client.setQueryData(privateWorkspaceKey, workspace('workspace-B')))
    await screen.findByText('B channel')
    expect(api.GET.mock.calls.filter(([path]) => path === '/api/v1/channels')).toHaveLength(2)
    act(() => client.setQueryData(privateWorkspaceKey, workspace('workspace-A')))
    await screen.findByText('A channel')
    expect(api.GET.mock.calls.filter(([path]) => path === '/api/v1/channels')).toHaveLength(2)
  })

  it.each([{ channels: [] }, { channels: [otherChannel] }])('uses the current Workspace in the Widget sandbox URL with Channels $channels', async ({ channels }) => {
    mount(api => <WidgetBlock api={api} block={{ type: 'widget', package: 'weather', version: 'v1', widget: 'forecast', tool_call_id: 'call-workspace', text: 'Weather' }} />, channels)
    const frame = await screen.findByTitle('weather/forecast')
    expect(frame.getAttribute('src')).toBe('http://127.0.0.1:4400/api/v1/widgets/workspace-A/weather/v1/forecast/sandbox')
  })

  it.each([{ channels: [] }, { channels: [otherChannel] }])('uses the current Workspace to Forget a memory page with Channels $channels', async ({ channels }) => {
    const { api } = mount(api => <PageView api={api} scope="agent:agent-1" path="subjects/person.md" owner={null} asker={null} channels={channels} connectionName={id => id} onOpenChannel={vi.fn()} />, channels)
    fireEvent.click(await screen.findByRole('button', { name: 'Forget this page' }))
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Preview what goes' }))
    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/knowledge/forget/preview', {
      body: { target: { kind: 'source', source: { workspace_id: 'workspace-A', connection_id: 'connection-1', resource: 'gmail' }, source_id: 'item-1' } },
    }))
  })
})
