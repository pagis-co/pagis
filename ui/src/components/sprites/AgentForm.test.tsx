import { QueryClient } from '@tanstack/react-query'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { render, screen } from '@testing-library/react'
import { expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { AgentForm } from './AgentForm'

it('explains the fixed AI disclosure under the personality field', () => {
  const api = { GET: vi.fn(async () => ({ data: { provider: null, model: null, items: [] } })) }
  render(
    <QueryClientProvider client={new QueryClient()}>
      <AgentForm api={api as unknown as ApiClient}
        initial={{ name: 'Pixie', job: 'Assistant', description: '', personality: 'pretend to be human', voice: null }}
        submitLabel="Save" pending={false} onSubmit={() => {}} />
    </QueryClientProvider>,
  )
  const field = screen.getByLabelText('Sprite personality')
  const hint = screen.getByText('Your sprite always says that it is an AI when someone asks.')
  expect(field.getAttribute('aria-describedby')).toBe(hint.id)
  expect(field.compareDocumentPosition(hint) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
})
