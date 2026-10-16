import { QueryClient } from '@tanstack/react-query'
import { render, screen, fireEvent } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import { QueryClientProvider } from '../test/QueryClientProvider'
import type { ApiClient } from '../api/client'
import { SearchBox, SearchResults } from './Search'

describe('Search', () => {
  it('submits one query and rejects short input', () => {
    const search = vi.fn()
    render(<SearchBox onSearch={search} />)
    const input = screen.getByRole('searchbox', { name: 'Search everywhere' })
    fireEvent.change(input, { target: { value: 'x' } })
    fireEvent.submit(input.closest('form')!)
    expect(search).not.toHaveBeenCalled()
    fireEvent.change(input, { target: { value: '  launch  ' } })
    fireEvent.submit(input.closest('form')!)
    expect(search).toHaveBeenCalledWith('launch')
  })

  it('labels results and names incomplete Workspaces, and opens the selected item', async () => {
    const item = { workspace: { id: 'team', name: 'Support', kind: 'shared' }, kind: 'calendar', title: 'Launch review', snippet: 'Read the plan', at: 100, url: '/calendar?event=event' }
    const get = vi.fn().mockResolvedValue({ data: { items: [item], incomplete: [{ workspace_id: 'private', workspace_name: 'Private Workspace', reason: 'timeout' }] } })
    const open = vi.fn()
    render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <SearchResults api={{ GET: get } as unknown as ApiClient} query="launch" onOpen={open} />
    </QueryClientProvider>)
    expect(await screen.findByText('Support')).toBeTruthy()
    expect(screen.getByText('Calendar Event')).toBeTruthy()
    expect(screen.getByRole('status').textContent).toContain('Private Workspace')
    fireEvent.click(screen.getByRole('link', { name: 'Launch review' }))
    expect(open).toHaveBeenCalledWith(item)
    expect(get).toHaveBeenCalledWith('/api/v1/search', { params: { query: { q: 'launch' } } })
  })
})
