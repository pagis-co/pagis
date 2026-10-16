import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ApiClient } from '../../api/client'
import { useColleagues } from '../../queries'
import { useCurrentWorkspaceId, useWorkspaceAccess } from '../../workspace'
import { Button, Frame, Row, SectionLabel, Select } from '../../primitives'

type Role = 'manager' | 'responder' | 'reader'
const roles = [
  { value: 'manager', label: 'Manager' },
  { value: 'responder', label: 'Responder' },
  { value: 'reader', label: 'Reader' },
]

export function Members({ api }: { api: ApiClient }) {
  const workspaceId = useCurrentWorkspaceId()
  const { canManageMembers } = useWorkspaceAccess()
  const colleagues = useColleagues(api)
  const client = useQueryClient()
  const key = ['workspace', workspaceId, 'members']
  const [person, setPerson] = useState('')
  const [role, setRole] = useState<Role>('reader')
  const members = useQuery({
    queryKey: key,
    enabled: canManageMembers,
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/workspaces/{workspace_id}/members', {
        params: { path: { workspace_id: workspaceId } },
      })
      if (!data) throw error ?? new Error('Could not read Members.')
      return data.items
    },
  })
  const change = useMutation({
    mutationFn: async (action: { kind: 'add' | 'change'; personId: string; role: Role } | { kind: 'remove'; personId: string }) => {
      const params = { path: { workspace_id: workspaceId, person_id: action.personId } }
      const response = action.kind === 'add'
        ? await api.POST('/api/v1/workspaces/{workspace_id}/members', {
          params: { path: { workspace_id: workspaceId } },
          body: { person_id: action.personId, role: action.role },
        })
        : action.kind === 'change'
          ? await api.PATCH('/api/v1/workspaces/{workspace_id}/members/{person_id}', {
            params, body: { role: action.role },
          })
          : await api.DELETE('/api/v1/workspaces/{workspace_id}/members/{person_id}', { params })
      if (response.error) throw new Error(response.error.error.message)
    },
    onSuccess: () => {
      setPerson('')
      void client.invalidateQueries({ queryKey: key })
      void client.invalidateQueries({ queryKey: ['workspaces'] })
    },
  })
  if (!canManageMembers) return null
  return (
    <section className="settings-section" aria-label="Members">
      <h1>Members</h1>
      <SectionLabel>People and roles</SectionLabel>
      <Frame>
        {(members.data ?? []).map((member) => {
          const name = member.name ?? member.email ?? member.person_id
          return (
            <Row key={member.person_id}>
              <span>{name}</span>
              <Select label={`Role for ${name}`} value={member.role}
                disabled={change.isPending}
                onValueChange={(value) => change.mutate({ kind: 'change', personId: member.person_id, role: value as Role })}
                items={roles} />
              <Button disabled={change.isPending} aria-label={`Remove ${name}`}
                onClick={() => change.mutate({ kind: 'remove', personId: member.person_id })}>
                Remove
              </Button>
            </Row>
          )
        })}
      </Frame>
      <form onSubmit={(event) => {
        event.preventDefault()
        change.mutate({ kind: 'add', personId: person.trim(), role })
      }}>
        <Select label="Person" value={person} onValueChange={setPerson}
          items={(colleagues.data ?? []).filter((candidate) =>
            !members.data?.some((member) => member.person_id === candidate.person_id),
          ).map((candidate) => ({ value: candidate.person_id, label: candidate.name }))} />
        <Select label="Role" value={role} onValueChange={(value) => setRole(value as Role)}
          items={roles} />
        <Button type="submit" disabled={!person.trim() || change.isPending}>Add Person</Button>
      </form>
      {members.isError && <p role="alert">Could not read Members.</p>}
      {change.isError && <p role="alert">{change.error.message}</p>}
    </section>
  )
}
