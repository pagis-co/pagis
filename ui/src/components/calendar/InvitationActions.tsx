import { useMutation, useQueryClient } from '@tanstack/react-query'
import type { ApiClient } from '../../api/client'
import { Button } from '../../primitives'
import { useWorkspaceMailbox, workspaceMailKey } from '../../queries'
import { useCurrentWorkspaceId } from '../../workspace'
import { calendarKey, type CalendarEvent } from './queries'

const responseLabels = {
  'needs-action': 'Not answered',
  accepted: 'Accepted',
  tentative: 'Maybe',
  declined: 'Declined',
}

export function InvitationActions({ api, event }: { api: ApiClient; event: CalendarEvent }) {
  const workspaceId = useCurrentWorkspaceId()
  const client = useQueryClient()
  const mailbox = useWorkspaceMailbox(api)
  const response = useMutation({
    mutationFn: async (status: 'accepted' | 'tentative' | 'declined') => {
      const { data, error } = await api.POST('/api/v1/calendar/events/{event_id}/response', {
        params: { path: { event_id: event.id } }, body: { status },
      })
      if (!data) throw error ?? new Error('Could not send the invitation response.')
      return data
    },
    onSuccess: async () => {
      await Promise.all([
        client.invalidateQueries({ queryKey: calendarKey(workspaceId) }),
        client.invalidateQueries({ queryKey: workspaceMailKey(workspaceId) }),
      ])
    },
  })
  const current = response.data ?? event
  const invitation = current.data.invitation
  if (!invitation) return null
  const own = mailbox.data?.mailbox?.address.toLowerCase()
  const attendee = current.data.attendees.find((a) => a.email.toLowerCase() === own)
  return (
    <section aria-label="Invitation">
      <p>{invitation.status === 'cancelled' ? 'Cancelled' : `Invitation · ${invitation.status}`}</p>
      {attendee && <p>Your response: {responseLabels[attendee.status]}</p>}
      {attendee && invitation.status !== 'cancelled' && own !== invitation.organizer.toLowerCase() && (
        <div className="mail-controls">
          {(['Accept', 'Maybe', 'Decline'] as const).map((label, index) => (
            <Button key={label} type="button" disabled={response.isPending} onClick={() => response.mutate((['accepted', 'tentative', 'declined'] as const)[index])}>{label}</Button>
          ))}
        </div>
      )}
      {response.isError && <p role="alert">Could not send the invitation response. Try again.</p>}
    </section>
  )
}
