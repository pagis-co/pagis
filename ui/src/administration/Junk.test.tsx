import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../api/client'
import { QueryClientProvider } from '../test/QueryClientProvider'
import { Junk } from './Junk'

it('reads quarantine content and sends an explicit release', async () => {
  const entry = { workspace_id: 'ws1', mailbox: 'person@example.com', message_key: '<unsafe@example.com>', host_id: 'INBOX:1', from: 'sender@example.com', subject: 'Unsafe', reasons: ['CLAM_VIRUS 20'], person_mark: null, quarantined: true, released: false }
  let released = false
  const GET = vi.fn(async (path: string) => ({ data: path.endsWith('junk-policy')
    ? { lists: { allow: [], block: [] }, sensitivity: 'normal' }
    : { items: released ? [] : [entry] } }))
  const POST = vi.fn(async (path: string) => {
    if (path.endsWith('/read')) return { data: { text: 'Quarantined body' } }
    released = true
    return {}
  })
  render(<QueryClientProvider client={new QueryClient()}><Junk api={{ GET, POST } as unknown as ApiClient} /></QueryClientProvider>)
  expect(screen.queryByText('Quarantined body')).toBeNull()
  fireEvent.click(await screen.findByRole('button', { name: 'Read Unsafe' }))
  expect(await screen.findByText('Quarantined body')).toBeTruthy()
  fireEvent.click(screen.getByRole('button', { name: 'Release Unsafe' }))
  await waitFor(() => expect(POST).toHaveBeenCalledWith('/api/v1/administration/mail/quarantine/release', { body: {
    workspace_id: 'ws1', mailbox: 'person@example.com', message_key: '<unsafe@example.com>',
  } }))
  expect(await screen.findByText('Quarantine is empty.')).toBeTruthy()
  expect(screen.queryByText('Quarantined body')).toBeNull()
})
