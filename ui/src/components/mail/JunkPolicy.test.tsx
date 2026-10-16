import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { WorkspaceJunkPolicy } from './JunkPolicy'

it('saves Workspace lists and moving mode', async () => {
  const GET = vi.fn(async () => ({ data: { lists: { allow: [], block: [] }, spam_mode: 'score_only' } }))
  const PUT = vi.fn(async () => ({}))
  render(<QueryClientProvider client={new QueryClient()}><WorkspaceJunkPolicy api={{ GET, PUT } as unknown as ApiClient} /></QueryClientProvider>)
  fireEvent.change(await screen.findByLabelText('Block list'), { target: { value: 'sender@example.com\nexample.com' } })
  fireEvent.click(screen.getByLabelText('Move junk to Junk'))
  fireEvent.click(screen.getByRole('button', { name: 'Save Junk settings' }))
  await waitFor(() => expect(PUT).toHaveBeenCalledWith('/api/v1/workspace/mail/junk-policy', { body: {
    lists: { allow: [], block: ['sender@example.com', 'example.com'] }, spam_mode: 'move_to_junk',
  } }))
})
