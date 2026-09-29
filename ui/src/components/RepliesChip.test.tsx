// The replies chip: under a message with replies, the faces of
// the authors, the count, the names and the time of the last reply.
// A click opens the reply thread.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import type { TimelineRow } from '../timeline'
import { RepliesChip } from './RepliesChip'

const lastReplyAt = Date.parse('2026-09-05T15:10:00Z')

function row(overrides: Partial<TimelineRow> = {}): TimelineRow {
  return {
    key: 'msg-1',
    kind: 'message',
    authorKind: 'agent',
    authorAgentId: 'ag-1',
    createdAt: Date.parse('2026-09-05T15:06:00Z'),
    sendState: 'sent',
    status: 'complete',
    runId: null,
    blocks: [{ type: 'markdown', text: 'hi' }],
    text: 'hi',
    completedAt: null,
    replyCount: 2,
    lastReplyAt,
    replyAuthors: [
      { authorKind: 'agent', agentId: 'ag-2' },
      { authorKind: 'user', agentId: null },
    ],
    ...overrides,
  }
}

function stubApi(userName: string | null = null) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/user') return { data: { name: userName } }
      return { data: { items: [{ id: 'ag-2', name: 'Clown', avatar: { sprite: 'pixie', preset: 'lavender', colors: {}, accessories: {} } }] } }
    }),
  } as unknown as ApiClient
}

function mount(r: TimelineRow, onOpen = () => {}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <RepliesChip api={stubApi()} row={r} onOpen={onOpen} />
    </QueryClientProvider>,
  )
}

const time = new Date(lastReplyAt).toLocaleTimeString(undefined, {
  hour: 'numeric',
  minute: '2-digit',
})

describe('RepliesChip', () => {
  it('names the count, the authors and the last reply time', async () => {
    mount(row())

    expect(
      await screen.findByRole('button', { name: `2 replies · Clown, you · ${time}` }),
    ).toBeTruthy()
  })

  it('draws one face for each author', async () => {
    const { container } = mount(row())
    await screen.findByRole('button', { name: /Clown/ })

    const faces = container.querySelectorAll('.ui-avatar-group .ui-avatar')
    expect(faces[0].querySelector('[role=img]')?.getAttribute('aria-label')).toBe('Clown, Pixie avatar')
    expect(faces[0].querySelector('img')?.src).toContain('lavender.png')
    expect(faces[1].textContent).toBe('Y')
  })

  it('says one reply in the singular', async () => {
    mount(row({ replyCount: 1, replyAuthors: [{ authorKind: 'user', agentId: null }] }))

    expect(
      await screen.findByRole('button', { name: `1 reply · you · ${time}` }),
    ).toBeTruthy()
  })

  it('opens the thread', async () => {
    const onOpen = vi.fn()
    mount(row(), onOpen)

    fireEvent.click(await screen.findByRole('button', { name: /2 replies/ }))

    expect(onOpen).toHaveBeenCalledOnce()
  })

  it('draws the user with the owner face, not one of the Agent hues', () => {
    const api = stubApi()
    render(
      <QueryClientProvider client={new QueryClient()}>
        <RepliesChip api={api as unknown as ApiClient} row={row()} onOpen={() => {}} />
      </QueryClientProvider>,
    )

    const faces = document.querySelectorAll('.ui-avatar')
    const owner = [...faces].find((face) =>
      face.classList.contains('ui-avatar-owner'),
    )
    // The chip's ground is `--accent-soft`, which is also hue 0. A face
    // on a hue that matches its ground disappears, so the user takes
    // none of them.
    expect(owner).toBeTruthy()
    expect(owner?.className).not.toMatch(/ui-avatar-hue-/)
  })

  // The user reads as one person everywhere, so the chip takes the
  // same initial as the sidebar.
  it('draws the user with the initial of the recorded name', async () => {
    const api = stubApi('Ada')
    render(
      <QueryClientProvider client={new QueryClient()}>
        <RepliesChip api={api as unknown as ApiClient} row={row()} onOpen={() => {}} />
      </QueryClientProvider>,
    )

    await screen.findByText('A')
  })
})
