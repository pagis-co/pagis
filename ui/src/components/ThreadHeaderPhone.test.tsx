// The phone nav bar of a conversation holds "Search this conversation"
// and "Speak replies" in a More menu, and only the items that the
// conversation has: a conversation with no message has nothing to
// search, and one where the person does not post gets no replies.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { shellResponse } from '../test/appStub'
import { renderInRouter } from '../test/router'
import { ThreadHeader } from './ThreadHeader'

const lastMessage = { author_kind: 'agent', author_agent_id: 'agent-1', created_at: 1, text_content: 'Hello.' }

const channels = [
  { id: 'with-messages', kind: 'dm', title: 'Sage', agent_ids: ['agent-1'], user_member: true, last_message: lastMessage },
  { id: 'empty', kind: 'dm', title: 'Sage', agent_ids: ['agent-1'], user_member: true, last_message: null },
  { id: 'agents-only', kind: 'group', title: 'Sage and Nova', agent_ids: ['agent-1', 'agent-2'], user_member: false, last_message: lastMessage },
  { id: 'agents-only-empty', kind: 'group', title: 'Sage and Nova', agent_ids: ['agent-1', 'agent-2'], user_member: false, last_message: null },
]

function mount(channelId: string) {
  const api = {
    GET: vi.fn(async (path: string) => (path === '/api/v1/channels' ? { data: { items: channels } } : shellResponse(path))),
  } as unknown as ApiClient
  renderInRouter(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ThreadHeader api={api} channelId={channelId} onTogglePanel={() => {}} />
    </QueryClientProvider>,
  )
}

/** The items of the More menu, opened from the keyboard. */
async function moreItems(): Promise<string[]> {
  const user = userEvent.setup()
  const more = await screen.findByRole('button', { name: 'More' })
  more.focus()
  await user.keyboard('{Enter}')
  return (await screen.findAllByRole('menuitem')).map((item) => item.textContent ?? '')
}

beforeEach(() => {
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: query.includes('max-width'),
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  }))
})
afterEach(() => vi.unstubAllGlobals())

describe('the More menu of a conversation on the phone', () => {
  it('holds search and spoken replies where the conversation has both', async () => {
    mount('with-messages')

    expect(await moreItems()).toEqual(['Search this conversation', 'Speak replies'])
  })

  it('holds no search in a conversation with no message', async () => {
    mount('empty')

    expect(await moreItems()).toEqual(['Speak replies'])
  })

  it('holds no spoken replies where the person does not post', async () => {
    mount('agents-only')

    expect(await moreItems()).toEqual(['Search this conversation'])
  })

  it('shows no More menu when the conversation has neither', async () => {
    mount('agents-only-empty')

    await screen.findByRole('heading', { name: 'Sage and Nova' })
    expect(screen.queryByRole('button', { name: 'More' })).toBeNull()
  })
})
