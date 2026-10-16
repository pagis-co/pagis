import { QueryClient } from '@tanstack/react-query'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { DisclosureRules } from './DisclosureRules'

function setup(fail = false) {
  const rule = { asker_workspace_id: 'colleague', category: 'mail' }
  const api = {
    GET: vi.fn(async (path: string) => ({ data: path === '/api/v1/colleagues'
      ? { items: [{ workspace_id: 'colleague', name: 'Sam' }] }
      : { profile: 'I design tools.', rules: [rule] } })),
    PUT: vi.fn(async () => fail ? { error: new Error('failed') } : { data: { profile: 'New profile', rules: [rule] } }),
    POST: vi.fn(async () => ({ data: { profile: 'I design tools.', rules: [rule] } })),
    DELETE: vi.fn(async () => ({ data: { profile: 'I design tools.', rules: [] } })),
  }
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><DisclosureRules api={api as unknown as ApiClient} /></QueryClientProvider>)
  return api
}

describe('Disclosure Rules', () => {
  it('shows the defaults and saves the profile', async () => {
    const api = setup()
    const profile = await screen.findByRole('textbox', { name: 'Profile' })
    expect((profile as HTMLTextAreaElement).value).toBe('I design tools.')
    expect(screen.getByText(/Free and busy times/)).toBeTruthy()
    fireEvent.change(profile, { target: { value: 'New profile' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save profile' }))
    await waitFor(() => expect(api.PUT).toHaveBeenCalledWith('/api/v1/disclosure/profile', { body: { profile: 'New profile' } }))
  })
  it('adds an Org rule and removes a colleague rule', async () => {
    const api = setup()
    await screen.findByRole('textbox', { name: 'Profile' })
    fireEvent.click(screen.getByRole('button', { name: 'Add rule' }))
    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/disclosure/rules', { body: { asker_workspace_id: null, category: 'memory' } }))
    fireEvent.click(screen.getByRole('button', { name: 'Remove Mail for Sam' }))
    await waitFor(() => expect(api.DELETE).toHaveBeenCalledWith('/api/v1/disclosure/rules', { body: { asker_workspace_id: 'colleague', category: 'mail' } }))
  })
  it('shows a failed profile save', async () => {
    setup(true)
    const profile = await screen.findByRole('textbox', { name: 'Profile' })
    fireEvent.change(profile, { target: { value: 'New profile' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save profile' }))
    expect(await screen.findByRole('alert')).toBeTruthy()
  })
})
