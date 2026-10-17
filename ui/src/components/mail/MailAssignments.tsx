import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import type { ApiClient } from '../../api/client'
import { Button, Input } from '../../primitives'
import { useWorkspaceAccess, useCurrentWorkspaceId } from '../../workspace'

export function MailAssignments({ api, messageId }: { api: ApiClient; messageId: string }) {
  const workspace = useCurrentWorkspaceId()
  const page = useQuery({
    queryKey: ['mail-assignments', workspace, messageId],
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/workspace/mail/assignments', { params: { query: { message_id: messageId } } })
      if (!data) throw new Error(error?.error.message ?? 'Could not load assignments.')
      return data
    },
  })
  if (page.isError) return <p role="alert">Could not load assignments.</p>
  if (!page.data?.items.length) return null
  return <section aria-label="Mail Assignments">
    <h4>Assignments</h4>
    <ul>{page.data.items.map(item => <li key={item.id}>
      <p>{item.summary}</p>
      <p>{item.assignee} · {item.role} · {item.state.replaceAll('_', ' ')}</p>
      {item.note && <p>{item.note}</p>}
      <a href={`/conversations/${encodeURIComponent(item.channel_id)}`}>Open assignment</a>
    </li>)}</ul>
  </section>
}

export function AssignmentFeed({ api }: { api: ApiClient }) {
  const { isPrivate } = useWorkspaceAccess()
  const client = useQueryClient()
  const page = useQuery({
    queryKey: ['assignment-feed'],
    enabled: isPrivate,
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/mail-assignments/feed')
      if (!data) throw new Error(error?.error.message ?? 'Could not load FYI assignments.')
      return data
    },
  })
  const dismiss = useMutation({
    mutationFn: async (id: string) => {
      const { error } = await api.POST('/api/v1/mail-assignments/{assignment_id}/done', { params: { path: { assignment_id: id } } })
      if (error) throw new Error(error.error.message)
    },
    onSuccess: () => { void client.invalidateQueries({ queryKey: ['assignment-feed'] }) },
  })
  if (!isPrivate) return null
  return <>
    {page.isError && <p role="alert">Could not load FYI assignments.</p>}
    {dismiss.isError && <p role="alert">{dismiss.error.message}</p>}
    {!!page.data?.items.length && <ul aria-label="FYI assignments">{page.data.items.map(item => <li key={item.id}>
      <p>{item.summary}</p>
      <a href={`/conversations/${encodeURIComponent(item.channel_id)}`}>Open assignment</a>
      <Button disabled={dismiss.isPending} onClick={() => dismiss.mutate(item.id)}>Dismiss FYI</Button>
    </li>)}</ul>}
  </>
}

export function AssignmentSettings({ api }: { api: ApiClient }) {
  const workspace = useCurrentWorkspaceId()
  const client = useQueryClient()
  const page = useQuery({
    queryKey: ['assignment-settings', workspace],
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/workspace/mailbox/assignment-settings')
      if (!data) throw new Error(error?.error.message ?? 'Could not load assignment settings.')
      return data
    },
  })
  const [limit, setLimit] = useState<string>()
  const [minutes, setMinutes] = useState<string>()
  const save = useMutation({
    mutationFn: async () => {
      const value = limit ?? String(page.data?.assignment_limit ?? '')
      const { data, error } = await api.PUT('/api/v1/workspace/mailbox/assignment-settings', { body: {
        assignment_limit: value === '' ? null : Number(value),
        assignment_timeout_minutes: Number(minutes ?? page.data?.assignment_timeout_minutes),
      } })
      if (!data) throw new Error(error?.error.message ?? 'Could not save assignment settings.')
    },
    onSuccess: () => { void client.invalidateQueries({ queryKey: ['assignment-settings', workspace] }) },
  })
  if (page.isError) return <p role="alert">Could not load assignment settings.</p>
  if (!page.data) return <p>Loading assignment settings…</p>
  return <form onSubmit={event => { event.preventDefault(); save.mutate() }}>
    <h4>Mail Assignments</h4>
    <label>Assignment limit per member<Input type="number" min="1" value={limit ?? page.data.assignment_limit ?? ''} onChange={event => setLimit(event.target.value)} /></label>
    <p>An empty limit permits any number of open assignments.</p>
    <label>Assignment timeout in minutes<Input required type="number" min="1" value={minutes ?? page.data.assignment_timeout_minutes} onChange={event => setMinutes(event.target.value)} /></label>
    <Button type="submit" disabled={save.isPending}>Save assignment settings</Button>
    {save.isError && <p role="alert">{save.error.message}</p>}
  </form>
}

export function ConversationAssignment({ api, conversationId, personId }: { api: ApiClient; conversationId: string; personId?: string }) {
  const client = useQueryClient()
  const assignment = useQuery({
    queryKey: ['conversation-assignment', conversationId],
    enabled: personId !== undefined,
    queryFn: async () => {
      const { data } = await api.GET('/api/v1/conversations/{conversation_id}/assignment', { params: { path: { conversation_id: conversationId } } })
      return data ?? null
    },
  })
  const done = useMutation({
    mutationFn: async () => {
      if (!assignment.data) return
      const { error } = await api.POST('/api/v1/mail-assignments/{assignment_id}/done', { params: { path: { assignment_id: assignment.data.id } } })
      if (error) throw new Error(error.error.message)
    },
    onSuccess: () => { void client.invalidateQueries({ queryKey: ['conversation-assignment', conversationId] }) },
  })
  const row = assignment.data
  if (!row || personId === undefined || row.assignee !== personId) return null
  return <div>
    <p>{row.role} · {row.state.replaceAll('_', ' ')}</p>
    {(row.state === 'escalated' || row.role === 'fyi' && row.state === 'assigned') &&
      <Button disabled={done.isPending} onClick={() => done.mutate()}>Mark assignment done</Button>}
    {done.isError && <p role="alert">{done.error.message}</p>}
  </div>
}
