import userEvent from '@testing-library/user-event'
import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { CurrentWorkspaceContext } from '../../workspace'
import { Calendar } from './Calendar'
import { EventEditor } from './EventEditor'
import type { CalendarEvent } from './queries'

const event: CalendarEvent = {
  id: 'event-1',
  uid: 'uid-1',
  workspace_id: 'workspace-1',
  created_by: { kind: 'system' },
  data: {
    title: 'Review',
    description: '',
    location: '',
    start: '2026-03-09T09:00:00',
    end: '2026-03-09T10:00:00',
    all_day: false,
    timezone: 'UTC',
    rrule: null,
    exdates: [],
    attendees: [],
    reminders: [15],
    meeting_url: 'https://meet.example.com/review',
  },
}

for (const view of ['Week', 'Month']) {
  it(`shows occurrences in the ${view.toLowerCase()} view`, async () => {
    const GET = vi.fn(async () => ({
      data: [{ event, start: '2026-03-09T09:00:00+00:00', end: '2026-03-09T10:00:00+00:00' }],
    }))
    render(
      <QueryClientProvider client={new QueryClient()}>
        <CurrentWorkspaceContext value="workspace-1">
          <Calendar api={{ GET } as unknown as ApiClient} initialDate="2026-03-09" />
        </CurrentWorkspaceContext>
      </QueryClientProvider>,
    )
    fireEvent.click(screen.getByRole('button', { name: view }))
    expect(await screen.findByRole('button', { name: /Review/ })).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Join meeting' }).getAttribute('href'))
      .toBe(event.data.meeting_url)
    expect(GET).toHaveBeenCalledWith(
      '/api/v1/calendar/events',
      expect.objectContaining({
        params: { query: { from: expect.any(Number), to: expect.any(Number) } },
      }),
    )
  })
}

it('creates an event with the editor fields', async () => {
  const POST = vi.fn(async () => ({ data: event }))
  const close = vi.fn()
  render(
    <QueryClientProvider client={new QueryClient()}>
      <CurrentWorkspaceContext value="workspace-1">
        <EventEditor api={{ POST } as unknown as ApiClient} date="2026-03-09" onClose={close} />
      </CurrentWorkspaceContext>
    </QueryClientProvider>,
  )
  fireEvent.change(screen.getByLabelText('Title'), { target: { value: 'Review' } })
  fireEvent.change(screen.getByLabelText('Time zone'), { target: { value: 'America/New_York' } })
  const user = userEvent.setup()
  await user.click(screen.getByRole('combobox', { name: 'Repeat' }))
  await user.click(await screen.findByRole('option', { name: 'Weekly' }))
  fireEvent.change(screen.getByLabelText('Meeting link'), { target: { value: 'https://meet.example.com/review' } })
  fireEvent.change(screen.getByLabelText('Reminder minutes'), { target: { value: '10, 30' } })
  fireEvent.click(screen.getByRole('button', { name: 'Add attendee' }))
  fireEvent.change(screen.getByLabelText('Attendee email 1'), { target: { value: 'reader@example.com' } })
  fireEvent.click(screen.getByRole('button', { name: 'Save event' }))
  await waitFor(() => expect(POST).toHaveBeenCalledWith('/api/v1/calendar/events', {
    body: expect.objectContaining({
      title: 'Review',
      timezone: 'America/New_York',
      rrule: 'FREQ=WEEKLY',
      reminders: [10, 30],
      meeting_url: 'https://meet.example.com/review',
      attendees: [{ email: 'reader@example.com', name: '', status: 'needs-action' }],
    }),
  }))
  await waitFor(() => expect(close).toHaveBeenCalled())
})

it('changes a series and deletes one occurrence through EXDATE', async () => {
  const PUT = vi.fn(async () => ({ data: event }))
  const repeating = { ...event, data: { ...event.data, rrule: 'FREQ=WEEKLY' } }
  render(
    <QueryClientProvider client={new QueryClient()}>
      <CurrentWorkspaceContext value="workspace-1">
        <EventEditor
          api={{ PUT } as unknown as ApiClient}
          date="2026-03-09"
          event={repeating}
          occurrenceStart="2026-03-16T09:00:00+00:00"
          onClose={vi.fn()}
        />
      </CurrentWorkspaceContext>
    </QueryClientProvider>,
  )
  fireEvent.click(screen.getByRole('button', { name: 'Delete this occurrence' }))
  await waitFor(() => expect(PUT).toHaveBeenCalledWith('/api/v1/calendar/events/{event_id}', {
    params: { path: { event_id: 'event-1' } },
    body: { ...repeating.data, exdates: ['2026-03-16T09:00:00'] },
  }))
})

it('shows the server refusal when an Event is under Legal Hold', async () => {
  const message = 'This Workspace is under legal hold. Nothing in it can be deleted until the hold ends.'
  const DELETE = vi.fn(async () => ({ error: { error: { code: 'conflict', message } } }))
  const close = vi.fn()
  render(<QueryClientProvider client={new QueryClient()}>
    <EventEditor api={{ DELETE } as unknown as ApiClient} event={event} date="2026-03-09" onClose={close} />
  </QueryClientProvider>)
  fireEvent.click(screen.getByRole('button', { name: 'Delete event' }))
  await waitFor(() => expect(screen.getAllByRole('alert').map(alert => alert.textContent)).toContain(message))
  expect(close).not.toHaveBeenCalled()
})

it('opens a Calendar Event from a Search result', async () => {
  const GET = vi.fn(async (path: string) => ({ data: path.endsWith("{event_id}") ? event : [] }))
  render(<QueryClientProvider client={new QueryClient()}><CurrentWorkspaceContext value="workspace-1">
    <Calendar api={{ GET } as unknown as ApiClient} initialDate="2026-03-09" eventId="event-1" />
  </CurrentWorkspaceContext></QueryClientProvider>)
  expect(await screen.findByRole('dialog')).toBeTruthy()
  expect(GET).toHaveBeenCalledWith('/api/v1/calendar/events/{event_id}', { params: { path: { event_id: 'event-1' } } })
})
