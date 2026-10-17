import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { MailTriageRules } from './MailTriageRules'

it('replaces the ordered sender rules', async () => {
  const GET = vi.fn(async () => ({ data: [{ sender: 'sender@example.com', action: 'needs_you' }] }))
  const PUT = vi.fn(async (_path, options) => ({ data: options.body }))
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><MailTriageRules api={{ GET, PUT } as unknown as ApiClient} /></QueryClientProvider>)
  expect(await screen.findByLabelText('Sender 1')).toHaveProperty('value', 'sender@example.com')
  fireEvent.click(screen.getByRole('button', { name: 'Add rule' }))
  fireEvent.change(screen.getByLabelText('Sender 2'), { target: { value: 'example.com' } })
  fireEvent.click(screen.getAllByRole('button', { name: 'Move up' })[1])
  fireEvent.click(screen.getByRole('button', { name: 'Save Mail Triage Rules' }))
  await waitFor(() => expect(PUT).toHaveBeenCalledWith('/api/v1/workspace/mailbox/triage-rules', { body: [
    { sender: 'example.com', action: 'needs_you' }, { sender: 'sender@example.com', action: 'needs_you' },
  ] }))
})
