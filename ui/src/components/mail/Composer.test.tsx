import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { Composer } from './Composer'

it('sends plain text mail to the entered recipients', async () => {
  const POST = vi.fn(async () => ({ data: { message_id: '<sent@test>' } }))
  const close = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Composer api={{ POST } as unknown as ApiClient} onClose={close} />
    </QueryClientProvider>,
  )
  fireEvent.change(screen.getByLabelText('To'), {
    target: { value: 'reader@test' },
  })
  fireEvent.change(screen.getByLabelText('Subject'), {
    target: { value: 'Hello' },
  })
  fireEvent.change(screen.getByLabelText('Body'), {
    target: { value: '<b>Hello</b>' },
  })
  fireEvent.click(screen.getByRole('button', { name: 'Send' }))
  await waitFor(() =>
    expect(POST).toHaveBeenCalledWith(
      '/api/v1/workspace/mail/messages',
      expect.objectContaining({
        body: expect.objectContaining({
          to: ['reader@test'],
          body: '<b>Hello</b>',
        }),
      }),
    ),
  )
  await waitFor(() => expect(close).toHaveBeenCalled())
})

vi.mock('../../api/client', async (original) => ({
  ...(await original<typeof import('../../api/client')>()),
  uploadFile: vi.fn(async () => ({ id: 'file-1' })),
}))

for (const action of ['reply', 'reply-all', 'forward'] as const) {
  it(`prefills ${action} and sends it through its message route`, async () => {
    const initial = {
      to:
        action === 'forward'
          ? []
          : action === 'reply-all'
            ? ['sender@test', 'other@test']
            : ['sender@test'],
      cc: action === 'reply-all' ? ['copy@test'] : [],
      subject: action === 'forward' ? 'Fwd: Hello' : 'Re: Hello',
      body: '\n\n> Hello',
    }
    const GET = vi.fn(async () => ({ data: initial }))
    const POST = vi.fn(async () => ({ data: { message_id: '<sent@test>' } }))
    render(
      <QueryClientProvider client={new QueryClient()}>
        <Composer
          api={{ GET, POST } as unknown as ApiClient}
          source={{ id: 'INBOX:1', action }}
          onClose={vi.fn()}
        />
      </QueryClientProvider>,
    )
    await waitFor(() =>
      expect((screen.getByLabelText('To') as HTMLInputElement).value).toBe(
        initial.to.join(', '),
      ),
    )
    expect((screen.getByLabelText('Cc') as HTMLInputElement).value).toBe(
      initial.cc.join(', '),
    )
    expect((screen.getByLabelText('Subject') as HTMLInputElement).value).toBe(
      initial.subject,
    )
    expect((screen.getByLabelText('Body') as HTMLInputElement).value).toBe(
      initial.body,
    )
    if (action === 'forward') {
      expect(
        screen.getByText('The original attachments are included.'),
      ).toBeTruthy()
      fireEvent.change(screen.getByLabelText('To'), {
        target: { value: 'recipient@test' },
      })
    }
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await waitFor(() =>
      expect(POST).toHaveBeenCalledWith(
        `/api/v1/workspace/mail/messages/{message_id}/${action}`,
        expect.objectContaining({
          params: { path: { message_id: 'INBOX:1' } },
        }),
      ),
    )
  })
}

it('uploads a File and sends its id as an attachment', async () => {
  const POST = vi.fn(async () => ({ data: { message_id: '<sent@test>' } }))
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Composer api={{ POST } as unknown as ApiClient} onClose={vi.fn()} />
    </QueryClientProvider>,
  )
  fireEvent.change(screen.getByLabelText('To'), {
    target: { value: 'recipient@test' },
  })
  fireEvent.change(screen.getByLabelText('Attach Files'), {
    target: {
      files: [new File(['invoice'], 'invoice.txt', { type: 'text/plain' })],
    },
  })
  expect(await screen.findByText('invoice.txt')).toBeTruthy()
  fireEvent.click(screen.getByRole('button', { name: 'Send' }))
  await waitFor(() =>
    expect(POST).toHaveBeenCalledWith(
      '/api/v1/workspace/mail/messages',
      expect.objectContaining({
        body: expect.objectContaining({ files: ['file-1'] }),
      }),
    ),
  )
})
