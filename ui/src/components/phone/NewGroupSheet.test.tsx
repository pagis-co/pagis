// The New group sheet: a name, a checkbox for each active Agent with its
// job, the count of the choice, and Create, which makes the group and
// opens it once.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { NewGroupSheet } from './NewGroupSheet'

function mount() {
  const post = vi.fn(async () => ({ data: { id: 'channel-9', kind: 'group', title: 'Austin trip', agent_ids: ['agent-1'] } }))
  const api = {
    GET: vi.fn(async (path: string) =>
      path === '/api/v1/agents'
        ? {
            data: {
              items: [
                { id: 'agent-1', name: 'Brownie', job: 'Travel planner', status: 'active' },
                { id: 'agent-2', name: 'Pixie', job: 'Finance clerk', status: 'active' },
                { id: 'agent-3', name: 'Pebble', job: 'Archivist', status: 'archived' },
              ],
            },
          }
        : { data: { items: [] } },
    ),
    POST: post,
  } as unknown as ApiClient
  const onClose = vi.fn()
  const onCreated = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <NewGroupSheet api={api} open onClose={onClose} onCreated={onCreated} />
    </QueryClientProvider>,
  )
  return { post, onClose, onCreated }
}

function create(): HTMLButtonElement {
  return within(screen.getByRole('dialog', { name: 'New group' })).getByRole('button', { name: 'Create' }) as HTMLButtonElement
}

describe('NewGroupSheet', () => {
  it('turns Create off until the group has a name and a sprite', async () => {
    mount()
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Brownie' }))
    expect(create().disabled).toBe(true)

    fireEvent.change(screen.getByLabelText('Group name'), { target: { value: 'Austin trip' } })

    expect(create().disabled).toBe(false)
  })

  it('offers each active sprite with its job', async () => {
    mount()

    expect(await screen.findByRole('checkbox', { name: 'Brownie' })).toBeTruthy()
    expect(screen.getByText('Travel planner')).toBeTruthy()
    expect(screen.getByRole('checkbox', { name: 'Pixie' })).toBeTruthy()
    expect(screen.queryByRole('checkbox', { name: 'Pebble' })).toBeNull()
  })

  it('counts the chosen sprites', async () => {
    mount()

    fireEvent.click(await screen.findByRole('checkbox', { name: 'Brownie' }))
    expect(screen.getByText(/^1 sprite\./)).toBeTruthy()

    fireEvent.click(screen.getByRole('checkbox', { name: 'Pixie' }))
    expect(screen.getByText(/^2 sprites\./)).toBeTruthy()
  })

  it('makes the group and opens it once', async () => {
    const { post, onClose, onCreated } = mount()
    fireEvent.change(screen.getByLabelText('Group name'), { target: { value: ' Austin trip ' } })
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Brownie' }))

    fireEvent.click(create())

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith('channel-9'))
    expect(post).toHaveBeenCalledTimes(1)
    expect(post).toHaveBeenCalledWith('/api/v1/channels', { body: { title: 'Austin trip', agent_ids: ['agent-1'] } })
    expect(onCreated).toHaveBeenCalledTimes(1)
    expect(onClose).not.toHaveBeenCalled()
  })
})
