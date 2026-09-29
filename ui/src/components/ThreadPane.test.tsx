// The thread pane: renders the root and its replies, and the
// composer sends into the thread (`parent_message_id`).

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, MessageDto } from '../api/client'
import { ThreadPane } from './ThreadPane'

function message(overrides: Partial<MessageDto>): MessageDto {
  const merged = {
    id: 'msg-1',
    channel_id: 'ch-1',
    author_kind: 'user',
    status: 'complete',
    text_content: 'hi',
    created_at: 1000,
    ...overrides,
  }
  return {
    ...merged,
    blocks: [{ type: 'markdown', text: merged.text_content }],
  }
}

function stubApi() {
  const thread = {
    root: message({ id: 'root-1', text_content: 'the root' }),
    replies: [
      message({
        id: 'reply-1',
        parent_message_id: 'root-1',
        author_kind: 'agent',
        text_content: 'first reply',
      }),
    ],
  }
  return {
    GET: vi.fn(async () => ({ data: thread })),
    POST: vi.fn(async () => ({
      data: message({
        id: 'reply-2',
        parent_message_id: 'root-1',
        text_content: 'typed reply',
        pending_id: 'p-1',
      }),
    })),
  }
}

function mount(
  api: ReturnType<typeof stubApi>,
  onClose = () => {},
  onOpenDesk = () => {},
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <ThreadPane
        api={api as unknown as ApiClient}
        channelId="ch-1"
        rootId="root-1"
        onClose={onClose}
        onOpenDesk={onOpenDesk}
      />
    </QueryClientProvider>,
  )
}

describe('ThreadPane', () => {
  it('renders the root and its replies', async () => {
    const api = stubApi()
    mount(api)

    expect(await screen.findByText('the root')).toBeTruthy()
    expect(await screen.findByText('first reply')).toBeTruthy()
    // The count sits in the header.
    expect(within(screen.getByTestId('thread-pane-header')).getByText('1 reply')).toBeTruthy()
    expect(api.GET).toHaveBeenCalledWith(
      '/api/v1/channels/{channel_id}/threads/{root_message_id}',
      { params: { path: { channel_id: 'ch-1', root_message_id: 'root-1' } } },
    )
  })

  it('sends the composer text as a thread reply', async () => {
    const api = stubApi()
    mount(api)
    await screen.findByText('the root')

    fireEvent.change(screen.getByPlaceholderText('Reply in thread'), {
      target: { value: 'typed reply' },
    })
    fireEvent.click(screen.getByText('Send'))

    await waitFor(() => expect(api.POST).toHaveBeenCalled())
    const [path, options] = api.POST.mock.calls[0] as unknown as [
      string,
      { body: { parent_message_id?: string; text: string } },
    ]
    expect(path).toBe('/api/v1/channels/{channel_id}/messages')
    expect(options.body.parent_message_id).toBe('root-1')
    expect(options.body.text).toBe('typed reply')
  })

  it('closes through the header button', async () => {
    const onClose = vi.fn()
    mount(stubApi(), onClose)
    await screen.findByText('the root')

    fireEvent.click(screen.getByLabelText('Close thread'))

    expect(onClose).toHaveBeenCalled()
  })

  // The thread sits over the Desk panel: the Desk button goes
  // back to it.
  it('goes back to the Desk through the header button', async () => {
    const onOpenDesk = vi.fn()
    mount(stubApi(), () => {}, onOpenDesk)
    await screen.findByText('the root')

    fireEvent.click(screen.getByRole('button', { name: 'Desk' }))

    expect(onOpenDesk).toHaveBeenCalled()
  })

  it('reads the root as prose, never as Markdown', async () => {
    const api = stubApi()
    api.GET = vi.fn(async () => ({
      data: {
        root: message({
          id: 'root-1',
          text_content: 'Yes, **Cascade Link** on the 900 plan.',
        }),
        replies: [],
      },
    })) as unknown as typeof api.GET
    mount(api)

    expect(await screen.findByText('Yes, Cascade Link on the 900 plan.')).toBeTruthy()
  })
})
