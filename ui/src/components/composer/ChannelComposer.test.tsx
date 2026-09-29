// The composer of an Agent-to-Agent Channel (ADR-0003): the user reads
// that Channel and writes nothing in it, so no user message can start
// an Agent-to-Agent exchange. Every Channel the user is in keeps its
// composer.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, ChannelDto } from '../../api/client'
import { ChannelComposer } from './ChannelComposer'

vi.mock('../Composer', () => ({
  Composer: ({ channelId }: { channelId: string }) => (
    <form data-testid="composer">{channelId}</form>
  ),
}))

function channel(overrides: Partial<ChannelDto>): ChannelDto {
  return {
    id: 'ch-sage',
    workspace_id: 'ws-1',
    kind: 'dm',
    agent_ids: ['ag-1'],
    user_member: true,
    title: 'Sage',
    created_at: 1,
    updated_at: 1,
    ...overrides,
  }
}

const channels = [
  channel({}),
  channel({ id: 'ch-ops', kind: 'group', agent_ids: ['ag-1', 'ag-2'], title: 'Ops' }),
  channel({
    id: 'ch-agents',
    agent_ids: ['ag-1', 'ag-2'],
    user_member: false,
    title: 'Sage ↔ Clown',
  }),
]

function mount(channelId: string) {
  const api = {
    GET: vi.fn(async () => ({ data: { items: channels } })),
  } as unknown as ApiClient
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  )
  render(<ChannelComposer api={api} channelId={channelId} />, { wrapper })
}

describe('ChannelComposer', () => {
  it('reads an Agent-to-Agent Channel without a composer', async () => {
    mount('ch-agents')

    expect(await screen.findByTestId('composer-read-only')).toHaveProperty(
      'textContent',
      'The sprites talk to each other here. You can read this conversation.',
    )
    expect(screen.queryByTestId('composer')).toBeNull()
  })

  it('writes in the user’s own Channel and in a group the user made', async () => {
    mount('ch-sage')
    expect(await screen.findByTestId('composer')).toBeTruthy()

    mount('ch-ops')
    expect((await screen.findAllByTestId('composer')).length).toBe(2)
    expect(screen.queryByTestId('composer-read-only')).toBeNull()
  })
})
