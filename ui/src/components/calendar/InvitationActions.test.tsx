import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { CurrentWorkspaceContext } from '../../workspace'
import type { CalendarEvent } from './queries'
import { InvitationActions } from './InvitationActions'

const event: CalendarEvent = {
  id: 'event-1', uid: 'meeting@example.org', workspace_id: 'workspace-1', created_by: { kind: 'system' },
  data: { title: 'Review', description: '', location: '', start: '2026-10-20T09:00:00', end: '2026-10-20T10:00:00', timezone: 'UTC', all_day: false, rrule: null, exdates: [], reminders: [], meeting_url: null,
    attendees: [{ email: 'reader@example.com', name: 'Reader', status: 'needs-action' }],
    invitation: { organizer: 'organizer@example.org', sequence: 1, status: 'tentative' },
  },
}
function mount(value = event, fail = false) {
  const POST = vi.fn(async () => fail ? { error: { message: 'Send failed' } } : { data: value })
  const GET = vi.fn(async () => ({ data: { mailbox: { address: 'reader@example.com' } } }))
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><CurrentWorkspaceContext value="workspace-1"><InvitationActions api={{ GET, POST } as unknown as ApiClient} event={value} /></CurrentWorkspaceContext></QueryClientProvider>)
  return POST
}
for (const [label, status] of [['Accept', 'accepted'], ['Maybe', 'tentative'], ['Decline', 'declined']]) {
  it(`sends ${status} from an invitation action`, async () => {
    const POST = mount()
    expect(await screen.findByText('Your response: Not answered')).toBeTruthy()
    fireEvent.click(await screen.findByRole('button', { name: label }))
    await waitFor(() => expect(POST).toHaveBeenCalledWith('/api/v1/calendar/events/{event_id}/response', { params: { path: { event_id: event.id } }, body: { status } }))
  })
}
it('shows a send error', async () => {
  mount(event, true)
  fireEvent.click(await screen.findByRole('button', { name: 'Accept' }))
  expect(await screen.findByRole('alert')).toBeTruthy()
})
it('shows cancellation without response actions', async () => {
  mount({ ...event, data: { ...event.data, invitation: { ...event.data.invitation!, status: 'cancelled' } } })
  expect(await screen.findByText('Cancelled')).toBeTruthy()
  expect(screen.queryByRole('button', { name: 'Accept' })).toBeNull()
})
