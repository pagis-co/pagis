import { QueryClient } from '@tanstack/react-query'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { QueryClientProvider } from '../test/QueryClientProvider'
import { renderInRouter } from '../test/router'
import { Mail } from './mail/Mail'

const device = vi.hoisted(() => ({ phone: false }))
vi.mock('../primitives/useMediaQuery', async (original) => ({
  ...(await original<typeof import('../primitives/useMediaQuery')>()),
  useIsMobile: () => device.phone,
}))
beforeEach(() => {
  device.phone = false
})

function setup(mailbox: object | null = { address: 'person@example.com' }) {
  const message = {
    id: 'INBOX:1',
    thread_id: '<thread@test>',
    from: 'sender@test',
    subject: 'Invoice',
    snippet: 'Please read',
    date: 1000,
    seen: false,
    labels: [],
    flagged: false,
    spam_verdict: { score: 7.2, action: 'add header', reasons: ['GTUBE'] },
  }
  let seen = false
  const GET = vi.fn(
    async (
      path: string,
      options?: {
        params?: { query?: { q?: string; cursor?: string } }
      },
    ) => {
      if (path === '/api/v1/workspace/mailbox')
        return { data: { mailbox, offers: [] } }
      if (path === '/api/v1/workspace/mail/folders')
        return { data: { items: ['INBOX'] } }
      if (path === '/api/v1/workspace/mail/messages') {
        const query = options?.params?.query
        return {
          data: {
            items:
              query?.q === 'absent'
                ? []
                : query?.cursor
                  ? [
                      {
                        ...message,
                        id: 'INBOX:0',
                        subject: 'Older invoice',
                        seen: true,
                      },
                    ]
                  : [{ ...message, seen }],
            next: query?.cursor ? null : 'older-page',
          },
        }
      }
      if (path === '/api/v1/workspace/mail/messages/{message_id}') {
        seen = true
        return { data: { ...message, text: 'invoice first', seen } }
      }
      if (path === '/api/v1/workspace/mail/threads/{thread_id}')
        return {
          data: {
            items: [
              { ...message, text: 'invoice first', seen },
              {
                ...message,
                id: 'INBOX:2',
                text: '<script>bad()</script>',
                seen,
              },
            ],
          },
        }
      throw new Error(path)
    },
  )
  renderInRouter(
    <QueryClientProvider
      client={
        new QueryClient({
          defaultOptions: { queries: { retry: false } },
        })
      }
    >
      <Mail api={{ GET } as unknown as ApiClient} />
    </QueryClientProvider>,
  )
  return GET
}

async function allMail() {
  fireEvent.click(await screen.findByRole('button', { name: 'All mail' }))
  return screen.findByRole('button', { name: /Invoice/ })
}

it('shows an empty Needs you view', async () => {
  setup()
  expect(await screen.findByText('No mail needs you.')).toBeTruthy()
})

it('lists sender, subject, snippet, time and the unread mark', async () => {
  setup()
  const row = await allMail()
  expect(within(row).getByText('sender@test')).toBeTruthy()
  expect(within(row).getByText('Please read')).toBeTruthy()
  expect(within(row).getByLabelText('Unread')).toBeTruthy()
  expect(row.querySelector('time')?.dateTime).toBe(new Date(1000).toISOString())
})

it('keeps the list beside the open thread on desktop and renders plain text', async () => {
  setup()
  fireEvent.click(await allMail())
  expect(await screen.findByText('invoice first')).toBeTruthy()
  expect(screen.getByRole('button', { name: /Invoice/ })).toBeTruthy()
  expect(screen.getByText('<script>bad()</script>')).toBeTruthy()
  expect(document.querySelector('script')).toBeNull()
})

it('shows one column on a phone and refreshes Seen when the reader goes back', async () => {
  device.phone = true
  const get = setup()
  fireEvent.click(await allMail())
  expect(await screen.findByText('invoice first')).toBeTruthy()
  expect(screen.queryByRole('button', { name: /Invoice/ })).toBeNull()
  const listCalls = () =>
    get.mock.calls.filter(
      ([path]) => path === '/api/v1/workspace/mail/messages',
    )
  expect(listCalls()).toHaveLength(1)
  fireEvent.click(screen.getByRole('button', { name: 'Mail' }))
  await waitFor(() => expect(listCalls()).toHaveLength(2))
  const row = await screen.findByRole('button', { name: /Invoice/ })
  await waitFor(() => expect(within(row).queryByLabelText('Unread')).toBeNull())
})

it('searches All mail', async () => {
  const get = setup()
  await allMail()
  fireEvent.change(screen.getByRole('searchbox'), {
    target: { value: 'absent' },
  })
  expect(await screen.findByText('No mail found.')).toBeTruthy()
  expect(get).toHaveBeenCalledWith('/api/v1/workspace/mail/messages', {
    params: { query: { folder: '', q: 'absent', cursor: undefined } },
  })
})

it('reads older-mail pages and returns to the newest page', async () => {
  const get = setup()
  await allMail()
  fireEvent.click(screen.getByRole('button', { name: 'Older mail' }))
  expect(
    await screen.findByRole('button', { name: /Older invoice/ }),
  ).toBeTruthy()
  expect(get).toHaveBeenCalledWith('/api/v1/workspace/mail/messages', {
    params: { query: { folder: '', q: '', cursor: 'older-page' } },
  })
  fireEvent.click(screen.getByRole('button', { name: 'Newest mail' }))
  expect(await screen.findByRole('button', { name: /Invoice/ })).toBeTruthy()
})

it('links to Settings when the Workspace has no mailbox', async () => {
  setup(null)
  const link = await screen.findByRole('link', {
    name: 'Set up your Workspace Mailbox',
  })
  expect(link.getAttribute('href')).toBe('/settings/mailbox')
})

it('shows the Spam Verdict with reasons on each message', async () => {
  setup()
  fireEvent.click(await allMail())
  expect(await screen.findAllByText('Spam score 7.2 · add header')).toHaveLength(2)
  expect(screen.getAllByText('GTUBE')).toHaveLength(2)
})
