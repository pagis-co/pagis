// The replies chip: under a message with replies, the faces of
// the authors, the count, the names and the time of the last reply.
// A click opens the reply thread.

import { QueryClient } from '@tanstack/react-query'
import { QueryClientProvider } from '../test/QueryClientProvider'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
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
    authorName: 'Ajay',
    authorId: 'ag-1',
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
      { authorKind: 'agent',
      authorName: 'Clown', authorId: 'ag-2' },
      { authorKind: 'person',
      authorName: 'Ajay', authorId: 'alice' },
    ],
    ...overrides,
  }
}

function stubApi(userName: string | null = null) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/user') return { data: { id: 'alice', name: userName } }
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
  it('keeps different Persons as separate reply authors', async () => {
    const { container } = mount(row({
      replyAuthors: [
        { authorKind: 'person', authorName: 'Alice', authorId: 'alice' },
        { authorKind: 'person', authorName: 'Bob', authorId: 'bob' },
      ],
    }))
    await waitFor(() => expect(container.querySelectorAll('.ui-avatar-owner')).toHaveLength(1))
    const faces = container.querySelectorAll('.ui-avatar')
    expect(faces[0].textContent).toBe('A')
    expect(faces[0].classList.contains('ui-avatar-owner')).toBe(true)
    expect(faces[1].textContent).toBe('B')
    expect(faces[1].classList.contains('ui-avatar-owner')).toBe(false)
  })

  it('names the count, the authors and the last reply time', async () => {
    mount(row())

    expect(
      await screen.findByRole('button', { name: `2 replies · Clown, Ajay · ${time}` }),
    ).toBeTruthy()
  })

  it('draws one face for each author', async () => {
    const { container } = mount(row())
    await screen.findByRole('button', { name: /Clown/ })

    const faces = container.querySelectorAll('.ui-avatar-group .ui-avatar')
    expect(faces[0].querySelector('[role=img]')?.getAttribute('aria-label')).toBe('Clown, Pixie avatar')
    await waitFor(() => expect(faces[0].querySelector('img')?.src).toContain('lavender.png'))
    expect(faces[1].textContent).toBe('A')
  })

  it('says one reply in the singular', async () => {
    mount(row({ replyCount: 1, replyAuthors: [{ authorKind: 'person',
      authorName: 'Ajay', authorId: 'alice' }] }))

    expect(
      await screen.findByRole('button', { name: `1 reply · Ajay · ${time}` }),
    ).toBeTruthy()
  })

  it('opens the thread', async () => {
    const onOpen = vi.fn()
    mount(row(), onOpen)

    fireEvent.click(await screen.findByRole('button', { name: /2 replies/ }))

    expect(onOpen).toHaveBeenCalledOnce()
  })

  it('draws the user with the owner face, not one of the Agent hues', async () => {
    const api = stubApi()
    render(
      <QueryClientProvider client={new QueryClient()}>
        <RepliesChip api={api as unknown as ApiClient} row={row()} onOpen={() => {}} />
      </QueryClientProvider>,
    )

    await waitFor(() => expect(document.querySelector('.ui-avatar-owner')).toBeTruthy())
    const owner = document.querySelector('.ui-avatar-owner')
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
