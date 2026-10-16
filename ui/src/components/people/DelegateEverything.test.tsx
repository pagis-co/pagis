import { QueryClient } from '@tanstack/react-query'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { it, expect, vi } from 'vitest'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import type { ApiClient } from '../../api/client'
import { DelegateEverything } from './DelegateEverything'

it('reads and saves Delegate everything', async () => {
  const put = vi.fn(async (_path, options) => ({ data: { id: 'workspace', delegate_everything: options.body.enabled } }))
  const api = { GET: vi.fn(async () => ({ data: { id: 'workspace', delegate_everything: false } })), PUT: put } as unknown as ApiClient
  render(<QueryClientProvider client={new QueryClient()}><DelegateEverything api={api} /></QueryClientProvider>)
  fireEvent.click(await screen.findByRole('checkbox', { name: 'Delegate everything' }))
  await waitFor(() => expect(put).toHaveBeenCalledWith('/api/v1/workspace/delegate-everything', { body: { enabled: true } }))
})
