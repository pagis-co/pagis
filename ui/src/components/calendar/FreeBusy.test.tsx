import { QueryClient } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { FreeBusy } from './FreeBusy'
import type { EventData } from './queries'

const draft: EventData = {
  title: 'Plan', description: '', location: '', start: '2026-03-08T09:00:00', end: '2026-03-08T10:00:00',
  timezone: 'America/New_York', all_day: false, rrule: null, exdates: [], reminders: [], meeting_url: null,
  attendees: [{ email: 'bob@example.com', name: 'Bob', status: 'accepted' }, { email: 'outside@example.com', name: 'Outside', status: 'needs-action' }],
}

it('shows own and Org attendee rows, a time axis and conflicts in the Event time zone', async () => {
  const POST = vi.fn(async () => ({ data: { items: [
    { address: 'alice@example.com', busy: [] },
    { address: 'bob@example.com', busy: [{ start: '2026-03-08T13:30:00Z', end: '2026-03-08T14:00:00Z' }] },
  ] } }))
  const api = { GET: vi.fn(async () => ({ data: { email: 'alice@example.com' } })), POST } as unknown as ApiClient
  const { rerender } = render(<QueryClientProvider client={new QueryClient()}><FreeBusy api={api} draft={draft} /></QueryClientProvider>)
  expect(await screen.findByRole('row', { name: /bob@example.com.*Conflict/ })).toBeTruthy()
  expect(screen.getByRole('row', { name: /alice@example.com/ })).toBeTruthy()
  expect(screen.queryByRole('row', { name: /outside@example.com/ })).toBeNull()
  expect(screen.getByText('09:00')).toBeTruthy()
  expect(screen.getAllByLabelText('Chosen time')).toHaveLength(2)
  await waitFor(() => expect(POST).toHaveBeenCalledWith('/api/v1/free-busy', { body: {
    from: '2026-03-08T05:00:00.000Z', to: '2026-03-09T04:00:00.000Z',
    addresses: ['alice@example.com', 'bob@example.com', 'outside@example.com'],
  } }))
  rerender(<QueryClientProvider client={new QueryClient()}><FreeBusy api={api} draft={{ ...draft, start: '2026-03-08T10:00:00', end: '2026-03-08T11:00:00' }} /></QueryClientProvider>)
  await waitFor(() => expect(screen.queryByText('Conflict')).toBeNull())
})

it('shows a failed read as unavailable', async () => {
  const api = { GET: vi.fn(async () => ({ data: { email: 'alice@example.com' } })), POST: vi.fn(async () => ({ error: new Error('failed') })) } as unknown as ApiClient
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><FreeBusy api={api} draft={draft} /></QueryClientProvider>)
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'Could not load Free/Busy.')
})

it('accepts an empty day while the Person edits the start', () => {
  const api = { GET: vi.fn(async () => ({ data: { email: 'alice@example.com' } })) } as unknown as ApiClient
  render(<QueryClientProvider client={new QueryClient()}><FreeBusy api={api} draft={{ ...draft, start: '' }} /></QueryClientProvider>)
  expect(screen.getByText('Enter a valid day and time zone.')).toBeTruthy()
})

it('does not mark an all-day Event as a conflict', async () => {
  const api = { GET: vi.fn(async () => ({ data: { email: 'alice@example.com' } })), POST: vi.fn(async () => ({ data: { items: [{ address: 'alice@example.com', busy: [{ start: '2026-03-08T13:00:00Z', end: '2026-03-08T14:00:00Z' }] }] } })) } as unknown as ApiClient
  render(<QueryClientProvider client={new QueryClient()}><FreeBusy api={api} draft={{ ...draft, all_day: true, start: '2026-03-08', end: '2026-03-09' }} /></QueryClientProvider>)
  expect(await screen.findByRole('row', { name: /alice@example.com/ })).toBeTruthy()
  expect(screen.queryByText('Conflict')).toBeNull()
  expect(screen.queryByLabelText('Chosen time')).toBeNull()
})
