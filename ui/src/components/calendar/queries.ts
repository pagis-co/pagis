import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ApiClient } from '../../api/client'
import type { components } from '../../api/schema'
import { useCurrentWorkspaceId } from '../../workspace'

export type CalendarEvent = components['schemas']['CalendarEvent']
export type EventData = components['schemas']['CalendarEventData']
export type Occurrence = components['schemas']['CalendarOccurrence']
export const calendarKey = (workspaceId: string) => ['workspace', workspaceId, 'calendar'] as const

export function useCalendar(api: ApiClient, from: number, to: number) {
  const workspaceId = useCurrentWorkspaceId()
  return useQuery({
    queryKey: [...calendarKey(workspaceId), from, to],
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/calendar/events', {
        params: { query: { from, to } },
      })
      if (data === undefined) throw error ?? new Error('Could not load Calendar.')
      return data
    },
  })
}

export function useSaveEvent(api: ApiClient, event?: CalendarEvent) {
  const workspaceId = useCurrentWorkspaceId()
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (body: EventData) => {
      const result = event
        ? await api.PUT('/api/v1/calendar/events/{event_id}', {
            params: { path: { event_id: event.id } },
            body,
          })
        : await api.POST('/api/v1/calendar/events', { body })
      if (result.data === undefined) throw result.error ?? new Error('Could not save Event.')
      return result.data
    },
    onSuccess: () => client.invalidateQueries({ queryKey: calendarKey(workspaceId) }),
  })
}

export function useDeleteEvent(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId()
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (id: string) => {
      const { error } = await api.DELETE('/api/v1/calendar/events/{event_id}', {
        params: { path: { event_id: id } },
      })
      if (error) throw error
    },
    onSuccess: () => client.invalidateQueries({ queryKey: calendarKey(workspaceId) }),
  })
}
