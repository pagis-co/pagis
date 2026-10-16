import type { ReactNode } from 'react'
import { QueryClient } from '@tanstack/react-query'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { beforeEach, describe, it, expect, vi } from 'vitest'
import { QueryClientProvider } from '../test/QueryClientProvider'
import type { ApiClient } from '../api/client'
import { People } from './People'
import { WorkspaceConversation } from './people/WorkspaceConversation'
import { usePendingSends } from '../state/stores'

const colleagues = [
  { person_id: 'b', workspace_id: 'wb', name: 'Bob', presence: 'active' },
  { person_id: 'c', workspace_id: 'wc', name: 'Carol', presence: 'away' },
]
function mount(element: ReactNode) {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      {element}
    </QueryClientProvider>,
  )
}
describe('People', () => {
  beforeEach(() => usePendingSends.setState({ byScope: {} }))

  it('shows a pending row and retries the same send after the draft changes', async () => {
    let fail!: (error: Error) => void
    const first = new Promise((_, reject) => {
      fail = reject
    })
    const messages: unknown[] = []
    const send = vi
      .fn()
      .mockImplementationOnce(() => first)
      .mockImplementation(async (_path, options) => {
        const message = {
          id: `m${messages.length}`,
          channel_id: 'conversation',
          author: { kind: 'person', id: 'alice', name: 'Alice' },
          text_content: options.body.text,
          blocks: [{ type: 'markdown', text: options.body.text }],
          pending_id: options.body.pending_id,
          status: 'complete',
          created_at: Date.now(),
          completed_at: Date.now(),
        }
        messages.unshift(message)
        return { data: message }
      })
    const api = {
      GET: vi.fn(async (path: string) => ({
        data:
          path === '/api/v1/user'
            ? { id: 'alice', name: 'Alice' }
            : { items: messages, next_cursor: null },
      })),
      POST: send,
    } as unknown as ApiClient
    mount(<WorkspaceConversation api={api} conversationId="conversation" />)
    const input = screen.getByRole('textbox', { name: 'Message' })
    fireEvent.change(input, { target: { value: 'First send' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(await screen.findByText('sending…')).toBeTruthy()
    expect(screen.getByText('First send')).toBeTruthy()
    expect((input as HTMLTextAreaElement).value).toBe('')
    const original = send.mock.calls[0][1].body
    fail(new Error('Connection closed'))
    const retry = await screen.findByRole('button', { name: 'Failed — retry' })
    fireEvent.change(input, { target: { value: 'Second send' } })
    fireEvent.click(retry)
    await waitFor(() => expect(send).toHaveBeenCalledTimes(2))
    expect(send.mock.calls[1][1].body).toEqual(original)
    await waitFor(() => expect(screen.queryByText('sending…')).toBeNull())
    expect(screen.getAllByText('First send')).toHaveLength(1)
    expect((input as HTMLTextAreaElement).value).toBe('Second send')
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await waitFor(() => expect(send).toHaveBeenCalledTimes(3))
    expect(send.mock.calls[2][1].body.pending_id).not.toBe(original.pending_id)
    expect(send.mock.calls[2][1].body.text).toBe('Second send')
  })

  it('lists colleagues with presence and starts a conversation with the selection', async () => {
    const start = vi.fn().mockResolvedValue({ data: { id: 'conversation' } })
    const open = vi.fn()
    const api = {
      GET: vi.fn(async (path: string) => ({
        data: { items: path === '/api/v1/colleagues' ? colleagues : [] },
      })),
      POST: start,
    } as unknown as ApiClient
    mount(<People api={api} onOpen={open} />)
    expect(await screen.findByText('Bob')).toBeTruthy()
    expect(screen.getByLabelText('Bob is active')).toBeTruthy()
    expect(screen.getByLabelText('Carol is away')).toBeTruthy()
    fireEvent.click(screen.getByRole('checkbox', { name: /Bob/ }))
    fireEvent.click(screen.getByRole('checkbox', { name: /Carol/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Start conversation' }))
    await waitFor(() =>
      expect(start).toHaveBeenCalledWith('/api/v1/conversations', {
        body: { workspace_ids: ['wb', 'wc'] },
      }),
    )
    expect(open).toHaveBeenCalledWith('conversation')
  })
  it('shows when a colleague is in a meeting', async () => {
    const api = { GET: vi.fn(async (path: string) => ({ data: { items: path === '/api/v1/colleagues' ? [{ ...colleagues[0], presence: 'in_a_meeting' }] : [] } })) } as unknown as ApiClient
    mount(<People api={api} onOpen={vi.fn()} />)
    expect(await screen.findByLabelText('Bob is in a meeting')).toBeTruthy()
    expect(screen.getByText('In a meeting')).toBeTruthy()
  })
  it('opens an existing conversation', async () => {
    const open = vi.fn()
    const api = {
      GET: vi.fn(async (path: string) => ({
        data: {
          items:
            path === '/api/v1/colleagues'
              ? colleagues
              : [
                  {
                    id: 'conversation',
                    participants: colleagues,
                    last_message: null,
                  },
                ],
        },
      })),
    } as unknown as ApiClient
    mount(<People api={api} onOpen={open} />)
    fireEvent.click(await screen.findByRole('button', { name: 'Bob, Carol' }))
    expect(open).toHaveBeenCalledWith('conversation')
  })
  it('shows the Person name on each reply and posts through the conversation route', async () => {
    const send = vi.fn().mockResolvedValue({
      data: {
        id: 'sent',
        channel_id: 'conversation',
        author: { kind: 'person', id: 'Alice', name: 'Alice' },
        blocks: [{ type: 'markdown', text: 'Hello' }],
        text_content: 'Hello',
        status: 'complete',
        created_at: 2,
        completed_at: 2,
      },
    })
    const api = {
      GET: vi.fn(async (path: string) => ({
        data:
          path === '/api/v1/user'
            ? { id: 'Alice', name: 'Alice' }
            : {
                items: ['Alice', 'Bob'].map((name, i) => ({
                  id: `m${i}`,
                  channel_id: 'conversation',
                  author: { kind: 'person', id: name, name },
                  text_content: `Hello ${name}`,
                  blocks: [{ type: 'markdown', text: `Hello ${name}` }],
                  status: 'complete',
                  created_at: i,
                  completed_at: i,
                })),
                next_cursor: null,
              },
      })),
      POST: send,
    } as unknown as ApiClient
    mount(<WorkspaceConversation api={api} conversationId="conversation" />)
    expect(await screen.findByText('Alice')).toBeTruthy()
    expect(screen.getByText('Bob')).toBeTruthy()
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Hello' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await waitFor(() =>
      expect(send).toHaveBeenCalledWith(
        '/api/v1/conversations/{conversation_id}/messages',
        expect.objectContaining({
          body: expect.objectContaining({ text: 'Hello' }),
        }),
      ),
    )
  })
})

it('loads older conversation pages to open a Search result', async () => {
  const message = (id: string, text: string) => ({ id, channel_id: 'conversation', author: { kind: 'person', id: 'alice', name: 'Alice' }, text_content: text, blocks: [{ type: 'markdown', text }], status: 'complete', created_at: 100, completed_at: 100 })
  const GET = vi.fn(async (path: string, options?: { params?: { query?: { before?: string } } }) => ({ data: path === '/api/v1/user'
    ? { id: 'alice', name: 'Alice' }
    : options?.params?.query?.before === 'cursor'
      ? { items: [message('older', 'Selected message')], next_cursor: null }
      : { items: [message('recent', 'Recent message')], next_cursor: 'cursor' } }))
  mount(<WorkspaceConversation api={{ GET } as unknown as ApiClient} conversationId="conversation" targetMessageId="older" />)
  expect(await screen.findByText('Selected message')).toBeTruthy()
  expect(screen.getByRole('region', { name: 'Selected message' })).toBeTruthy()
})
