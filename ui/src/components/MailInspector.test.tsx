// The mail inspector (ADR-0019): the headers and the body, read
// live through the daemon and stored nowhere.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import type { MailBlockDto } from '../blocks/mail'
import { MailInspector } from './MailInspector'

const inbound: MailBlockDto = {
  type: 'mail',
  direction: 'inbound',
  mailbox: 'ada@example.com',
  message_id: 'INBOX:12',
  counterpart: 'Clinic <care@clinic.test>',
  subject: 'Your appointment',
  trust_tier: 'unknown',
}

const message = {
  mailbox: 'ada@example.com',
  message_id: 'INBOX:12',
  from: 'Clinic <care@clinic.test>',
  to: ['ada@example.com'],
  subject: 'Your appointment',
  date: 1_000,
  headers: [
    { name: 'Subject', value: 'Your appointment' },
    { name: 'Message-ID', value: '<a1@clinic.test>' },
  ],
  text: 'Tuesday at nine twenty.',
  attachments: [{ name: 'card.pdf', bytes: 2_048 }],
}

function stubApi(answer: 'message' | 'gone') {
  return {
    GET: vi.fn(async () =>
      answer === 'message'
        ? { data: message }
        : { error: { error: { message: 'this message is no longer at the host' } } },
    ),
    POST: vi.fn(async () => ({ data: {} })),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({ data: {} })),
  }
}

function mount(answer: 'message' | 'gone', block: MailBlockDto = inbound) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  const onClose = vi.fn()
  render(
    <QueryClientProvider client={queryClient}>
      <MailInspector
        api={stubApi(answer) as unknown as ApiClient}
        mail={block}
        onClose={onClose}
      />
    </QueryClientProvider>,
  )
  return onClose
}

describe('the mail inspector', () => {
  it('reads the body of the message live', async () => {
    mount('message')
    const body = await screen.findByTestId('mail-body')
    expect(body.textContent).toContain('Tuesday at nine twenty.')
    expect(screen.getByTestId('mail-inspector').textContent).toContain(
      'It keeps no copy.',
    )
  })

  it('says the words came from outside Pagis', async () => {
    mount('message')
    await screen.findByTestId('mail-body')
    expect(screen.getByTestId('mail-inspector').textContent).toContain(
      'Somebody outside Pagis wrote this message',
    )
    expect(screen.getByTestId('mail-inspector').textContent).toContain(
      'This sender is not identified.',
    )
  })

  it('opens the headers behind one control', async () => {
    mount('message')
    const toggle = await screen.findByRole('button', { name: 'Read the headers' })
    expect(screen.queryByTestId('mail-headers')).toBeNull()
    fireEvent.click(toggle)
    expect(screen.getByTestId('mail-headers').textContent).toContain(
      '<a1@clinic.test>',
    )
  })

  it('lists an attachment by name and size, never its bytes', async () => {
    mount('message')
    await screen.findByTestId('mail-body')
    expect(screen.getByTestId('mail-inspector').textContent).toContain(
      'card.pdf · 2048 bytes',
    )
  })

  it('says so when the message is not at the host any more', async () => {
    mount('gone')
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain(
        'this message is no longer at the host',
      ),
    )
  })

  it('closes on the control', async () => {
    const onClose = mount('message')
    fireEvent.click(await screen.findByLabelText('Close the mail'))
    expect(onClose).toHaveBeenCalled()
  })
})
