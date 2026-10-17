import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import type { ApiClient } from '../../api/client'
import type { components } from '../../api/schema'
import { Button, Input, Select } from '../../primitives'
import { useCurrentWorkspaceId } from '../../workspace'

type Rule = components['schemas']['MailTriageRule']

export function MailTriageRules({ api }: { api: ApiClient }) {
  const workspace = useCurrentWorkspaceId()
  const client = useQueryClient()
  const key = ['mail-triage-rules', workspace]
  const query = useQuery({ queryKey: key, queryFn: async () => {
    const { data, error } = await api.GET('/api/v1/workspace/mailbox/triage-rules')
    if (!data) throw new Error(error?.error.message ?? 'Could not read Mail Triage Rules.')
    return data
  } })
  return <section>
    <h4>Mail Triage Rules</h4>
    <p>The first matching address or domain decides who handles the message. Domains do not include subdomains.</p>
    {query.isError && <p role="alert">Could not read Mail Triage Rules.</p>}
    {query.data && <RuleEditor api={api} initial={query.data} onSave={() => { void client.invalidateQueries({ queryKey: key }) }} />}
  </section>
}

function RuleEditor({ api, initial, onSave }: { api: ApiClient; initial: Rule[]; onSave: () => void }) {
  const [rules, setRules] = useState(initial)
  const save = useMutation({ mutationFn: async () => {
    const { data, error } = await api.PUT('/api/v1/workspace/mailbox/triage-rules', { body: rules })
    if (!data) throw new Error(error?.error.message ?? 'Could not save Mail Triage Rules.')
  }, onSuccess: onSave })
  function update(index: number, rule: Rule) { setRules(rules.map((item, i) => i === index ? rule : item)) }
  function move(index: number, offset: number) {
    const next = [...rules]
    ;[next[index], next[index + offset]] = [next[index + offset], next[index]]
    setRules(next)
  }
  return <form onSubmit={event => { event.preventDefault(); save.mutate() }}>
    <ol>{rules.map((rule, index) => <li key={index}>
      <label>Sender {index + 1}<Input required maxLength={254} value={rule.sender} onChange={event => update(index, { ...rule, sender: event.target.value })} /></label>
      <Select label={`Action ${index + 1}`} value={rule.action} onValueChange={value => update(index, { ...rule, action: value as Rule['action'] })} items={[
        { value: 'needs_you', label: 'Always show me' }, { value: 'agent', label: 'The Chief of Staff handles it alone' },
      ]} />
      <Button type="button" disabled={index === 0} onClick={() => move(index, -1)}>Move up</Button>
      <Button type="button" disabled={index === rules.length - 1} onClick={() => move(index, 1)}>Move down</Button>
      <Button type="button" onClick={() => setRules(rules.filter((_, i) => i !== index))}>Remove rule</Button>
    </li>)}</ol>
    <Button type="button" onClick={() => setRules([...rules, { sender: '', action: 'needs_you' }])}>Add rule</Button>
    <Button type="submit" disabled={save.isPending}>Save Mail Triage Rules</Button>
    {save.isError && <p role="alert">{save.error.message}</p>}
    {save.isSuccess && <p>Mail Triage Rules saved.</p>}
  </form>
}
