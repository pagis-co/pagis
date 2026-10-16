import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { FolderManager } from './FolderManager'
import { MessageControls } from './MessageControls'

it('creates a folder', async () => {
  const POST = vi.fn(async () => ({}))
  render(
    <QueryClientProvider client={new QueryClient()}>
      <FolderManager
        api={{ POST } as unknown as ApiClient}
        folders={['INBOX']}
      />
    </QueryClientProvider>,
  )
  fireEvent.change(screen.getByLabelText('Folder name'), {
    target: { value: 'Invoices' },
  })
  fireEvent.click(screen.getByRole('button', { name: 'Create folder' }))
  await waitFor(() =>
    expect(POST).toHaveBeenCalledWith('/api/v1/workspace/mail/folders', {
      body: { name: 'Invoices' },
    }),
  )
})

it('applies a label to a message', async () => {
  const POST = vi.fn(async () => ({}))
  const GET = vi.fn(async () => ({ data: { items: ['INBOX'] } }))
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MessageControls
        api={{ POST, GET } as unknown as ApiClient}
        id="INBOX:1"
        labels={[]}
      />
    </QueryClientProvider>,
  )
  fireEvent.change(screen.getByLabelText('Label'), {
    target: { value: 'finance' },
  })
  fireEvent.click(screen.getByRole('button', { name: 'Apply label' }))
  await waitFor(() =>
    expect(POST).toHaveBeenCalledWith(
      '/api/v1/workspace/mail/messages/{message_id}/labels',
      {
        params: { path: { message_id: 'INBOX:1' } },
        body: { name: 'finance' },
      },
    ),
  )
})

it('renames and deletes folders with a confirmation and protects system names', async () => {
  const PATCH = vi.fn(async () => ({}))
  const DELETE = vi.fn(async () => ({}))
  render(
    <QueryClientProvider client={new QueryClient()}>
      <FolderManager
        api={{ PATCH, DELETE } as unknown as ApiClient}
        folders={['INBOX', 'Invoices']}
      />
    </QueryClientProvider>,
  )
  expect(screen.queryByRole('button', { name: 'Delete INBOX' })).toBeNull()
  fireEvent.change(screen.getByLabelText('Rename Invoices'), {
    target: { value: 'Paid' },
  })
  fireEvent.click(screen.getByRole('button', { name: 'Rename Invoices' }))
  await waitFor(() =>
    expect(PATCH).toHaveBeenCalledWith(
      '/api/v1/workspace/mail/folders/{name}',
      { params: { path: { name: 'Invoices' } }, body: { name: 'Paid' } },
    ),
  )
  await waitFor(() =>
    expect(
      (
        screen.getByRole('button', {
          name: 'Delete Invoices',
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false),
  )
  fireEvent.click(screen.getByRole('button', { name: 'Delete Invoices' }))
  expect(DELETE).not.toHaveBeenCalled()
  fireEvent.click(
    screen.getByRole('button', { name: 'Confirm delete Invoices' }),
  )
  await waitFor(() =>
    expect(DELETE).toHaveBeenCalledWith(
      '/api/v1/workspace/mail/folders/{name}',
      { params: { path: { name: 'Invoices' } } },
    ),
  )
})

it('removes a label', async () => {
  const DELETE = vi.fn(async () => ({}))
  const GET = vi.fn(async () => ({ data: { items: ['INBOX'] } }))
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MessageControls
        api={{ GET, DELETE } as unknown as ApiClient}
        id="INBOX:1"
        labels={['finance']}
      />
    </QueryClientProvider>,
  )
  fireEvent.click(screen.getByRole('button', { name: 'Remove label finance' }))
  await waitFor(() =>
    expect(DELETE).toHaveBeenCalledWith(
      '/api/v1/workspace/mail/messages/{message_id}/labels/{label}',
      { params: { path: { message_id: 'INBOX:1', label: 'finance' } } },
    ),
  )
})

it('moves a message to the selected folder', async () => {
  const POST = vi.fn(async () => ({}))
  const GET = vi.fn(async () => ({ data: { items: ['INBOX', 'Paid'] } }))
  const moved = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MessageControls
        api={{ POST, GET } as unknown as ApiClient}
        id="INBOX:1"
        labels={[]}
        onMove={moved}
      />
    </QueryClientProvider>,
  )
  await waitFor(() => expect(GET).toHaveBeenCalled())
  fireEvent.keyDown(screen.getByRole('combobox', { name: 'Move to folder' }), {
    key: 'ArrowDown',
  })
  fireEvent.click(await screen.findByRole('option', { name: 'Paid' }))
  fireEvent.click(screen.getByRole('button', { name: 'Move' }))
  await waitFor(() =>
    expect(POST).toHaveBeenCalledWith(
      '/api/v1/workspace/mail/messages/{message_id}/move',
      { params: { path: { message_id: 'INBOX:1' } }, body: { name: 'Paid' } },
    ),
  )
  await waitFor(() => expect(moved).toHaveBeenCalled())
})

it('shows the server refusal for a held mail folder and keeps the folder', async () => {
  const message = 'This Workspace is under legal hold. Nothing in it can be deleted until the hold ends.'
  const DELETE = vi.fn(async () => ({ error: { error: { code: 'conflict', message } } }))
  render(<QueryClientProvider client={new QueryClient()}>
    <FolderManager api={{ DELETE } as unknown as ApiClient} folders={['Records']} />
  </QueryClientProvider>)
  fireEvent.click(screen.getByRole('button', { name: 'Delete Records' }))
  fireEvent.click(screen.getByRole('button', { name: 'Confirm delete Records' }))
  expect((await screen.findByRole('alert')).textContent).toBe(message)
  expect(screen.getByRole('button', { name: 'Delete Records' })).toBeTruthy()
})
