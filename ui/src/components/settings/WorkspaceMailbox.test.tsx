import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { WorkspaceMailbox } from './WorkspaceMailbox'

it('creates the Workspace Mailbox from the local part and Org mail domain', async () => {
  let mailbox: { address: string } | null = null
  const GET = vi.fn(async () => ({ data: { mailbox, offers: [{ connection_id: 'cn1', domain: 'example.com', password: 'minted' }] } }))
  const POST = vi.fn(async () => { mailbox = { address: 'person@example.com' }; return { data: mailbox } })
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><WorkspaceMailbox api={{ GET, POST } as unknown as ApiClient} /></QueryClientProvider>)
  fireEvent.change(await screen.findByLabelText('Local part'), { target: { value: 'person' } })
  fireEvent.click(screen.getByRole('button', { name: 'Create Workspace Mailbox' }))
  await waitFor(() => expect(POST).toHaveBeenCalledWith('/api/v1/workspace/mailbox', { body: { connection_id: 'cn1', local_part: 'person' } }))
  expect(await screen.findByText('person@example.com')).toBeTruthy()
  expect(screen.queryByRole('button', { name: 'Create Workspace Mailbox' })).toBeNull()
})

it('asks for the manual mailbox password and shows a refused create', async () => {
  const GET = vi.fn(async () => ({ data: { mailbox: null, offers: [{ connection_id: 'cn1', domain: 'example.com', password: 'typed' }] } }))
  const POST = vi.fn(async () => ({ error: { error: { code: 'conflict', message: 'The address is taken.' } } }))
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><WorkspaceMailbox api={{ GET, POST } as unknown as ApiClient} /></QueryClientProvider>)
  fireEvent.change(await screen.findByLabelText('Local part'), { target: { value: 'person' } })
  fireEvent.change(screen.getByLabelText('Mailbox password'), { target: { value: 'secret' } })
  fireEvent.click(screen.getByRole('button', { name: 'Create Workspace Mailbox' }))
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'The address is taken.')
  expect(POST).toHaveBeenCalledWith('/api/v1/workspace/mailbox', { body: { connection_id: 'cn1', local_part: 'person', password: 'secret' } })
})
