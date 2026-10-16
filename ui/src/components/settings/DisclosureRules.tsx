import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ApiClient } from '../../api/client'
import type { components } from '../../api/schema'
import { Button, Frame, Row, Select, Textarea } from '../../primitives'
import { errorMessage } from '../../queries'
import { SettingsSection } from './SettingsSection'
import './DisclosureRules.css'

type Rule = components['schemas']['DisclosureRule']
type Data = components['schemas']['DisclosureDto']
const categories: { value: Rule['category']; label: string }[] = [
  { value: 'memory', label: 'Memory' },
  { value: 'mail', label: 'Mail' },
  { value: 'calendar_details', label: 'Calendar details' },
  { value: 'other_private_data', label: 'Other private data' },
]
const key = ['disclosure']

export function DisclosureRules({ api }: { api: ApiClient }) {
  const disclosure = useQuery({ queryKey: key, queryFn: async () => {
    const { data, error } = await api.GET('/api/v1/disclosure')
    if (!data) throw error
    return data
  } })
  const colleagues = useQuery({ queryKey: ['colleagues'], queryFn: async () => {
    const { data, error } = await api.GET('/api/v1/colleagues')
    if (!data) throw error
    return data.items
  } })
  if (disclosure.isError || colleagues.isError) return <p role="alert">Disclosure Rules could not be loaded.</p>
  if (!disclosure.data || !colleagues.data) return <p>Loading Disclosure Rules…</p>
  return <Editor api={api} data={disclosure.data} colleagues={colleagues.data} />
}

function Editor({ api, data, colleagues }: { api: ApiClient; data: Data; colleagues: { workspace_id: string; name: string }[] }) {
  const client = useQueryClient()
  const [profile, setProfile] = useState(data.profile)
  const [asker, setAsker] = useState('org')
  const [category, setCategory] = useState<Rule['category']>('memory')
  const save = useMutation({ mutationFn: async () => {
    const result = await api.PUT('/api/v1/disclosure/profile', { body: { profile } })
    if (!result.data) throw result.error
    return result.data
  }, onSuccess: value => client.setQueryData(key, value) })
  const change = useMutation({ mutationFn: async ({ rule, remove }: { rule: Rule; remove: boolean }) => {
    const result = remove
      ? await api.DELETE('/api/v1/disclosure/rules', { body: rule })
      : await api.POST('/api/v1/disclosure/rules', { body: rule })
    if (!result.data) throw result.error
    return result.data
  }, onSuccess: value => client.setQueryData(key, value) })
  const name = (rule: Rule) => rule.asker_workspace_id == null ? 'Every Workspace of the Org'
    : colleagues.find(p => p.workspace_id === rule.asker_workspace_id)?.name ?? rule.asker_workspace_id
  return <SettingsSection title="Disclosure Rules" lead="Choose what your Chief of Staff can share with colleagues." hint="Free and busy times, your profile and Files already shared with the asker pass by default. Other private data needs an Approval or a Disclosure Rule. Collaborative Artifacts are not built.">
    <Frame>
      <Row><label htmlFor="disclosure-profile">Profile</label></Row>
      <Row><Textarea className="disclosure-profile" id="disclosure-profile" aria-label="Profile" value={profile} maxLength={16000} onChange={event => setProfile(event.target.value)} /></Row>
      <Row><Button disabled={save.isPending || profile === data.profile} onClick={() => save.mutate()}>Save profile</Button></Row>
      {save.isError && <Row><p role="alert">{errorMessage(save.error, 'The profile could not be saved.')}</p></Row>}
    </Frame>
    <Frame>
      {data.rules.length === 0 && <Row>No standing Disclosure Rules.</Row>}
      {data.rules.map(rule => {
        const label = `${categories.find(c => c.value === rule.category)?.label} for ${name(rule)}`
        return <Row className="disclosure-rule" key={`${rule.asker_workspace_id ?? 'org'}:${rule.category}`}><span>{label}</span><Button size="sm" aria-label={`Remove ${label}`} disabled={change.isPending} onClick={() => change.mutate({ rule, remove: true })}>Remove</Button></Row>
      })}
      <Row className="disclosure-rule-editor">
        <Select label="Asker" value={asker} onValueChange={setAsker} items={[{ value: 'org', label: 'Every Workspace of the Org' }, ...colleagues.map(p => ({ value: p.workspace_id, label: p.name }))]} />
        <Select label="Category" value={category} onValueChange={value => setCategory(value as Rule['category'])} items={categories} />
        <Button disabled={change.isPending} onClick={() => change.mutate({ rule: { asker_workspace_id: asker === 'org' ? null : asker, category }, remove: false })}>Add rule</Button>
      </Row>
      {change.isError && <Row><p role="alert">{errorMessage(change.error, 'The Disclosure Rule could not be saved.')}</p></Row>}
    </Frame>
  </SettingsSection>
}
