import { useMutation, useQueryClient } from '@tanstack/react-query'
import type { ApiClient } from '../../api/client'
import { Button } from '../../primitives'
import { useWorkspaceMailbox, workspaceMailKey, useUser } from '../../queries'
import { useCurrentWorkspaceId, useWorkspaceAccess } from '../../workspace'
import { calendarKey, type CalendarEvent } from './queries'

const responseLabels = {
  'needs-action': 'Not answered',
  accepted: 'Accepted',
  tentative: 'Maybe',
  declined: 'Declined',
}

export function InvitationActions({ api, event, messageId, memberInvitation = false }: { api: ApiClient; event: CalendarEvent; messageId?: string; memberInvitation?: boolean }) {
  const workspaceId = useCurrentWorkspaceId()
  const { canRespond, isPrivate } = useWorkspaceAccess()
  const client = useQueryClient()
  const mailbox = useWorkspaceMailbox(api)
  const person = useUser(api)
  const personal = memberInvitation || (!isPrivate && event.data.attendees.some(a => a.email.toLowerCase() === person.data?.email?.toLowerCase()))
  const response = useMutation({
    mutationFn: async (status: 'accepted' | 'tentative' | 'declined') => {
      const { data, error } = await (messageId
        ? api.POST('/api/v1/workspace/mail/messages/{message_id}/response', {
          params: { path: { message_id: messageId } }, body: { status },
        })
        : api.POST('/api/v1/calendar/events/{event_id}/response', {
          params: { path: { event_id: event.id } }, body: { status },
          ...(memberInvitation ? { pathSerializer: () => `/api/v1/workspaces/${encodeURIComponent(event.workspace_id)}/calendar/events/${encodeURIComponent(event.id)}/response` } : {}),
        }))
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
  if (!invitation && !personal) return null
  const own = personal ? person.data?.email?.toLowerCase() : mailbox.data?.mailbox?.address.toLowerCase()
  const attendee = current.data.attendees.find((a) => a.email.toLowerCase() === own)
  return (
    <section aria-label="Invitation">
      <p>{invitation?.status === 'cancelled' ? 'Cancelled' : `Invitation · ${invitation?.status ?? 'Team event'}`}</p>
      {attendee && <p>Your response: {responseLabels[attendee.status]}</p>}
      {(personal || (canRespond && attendee && own !== invitation?.organizer.toLowerCase())) && invitation?.status !== 'cancelled' && (
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
