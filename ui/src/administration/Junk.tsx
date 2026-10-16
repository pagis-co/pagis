import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import type { ApiClient } from '../api/client'
import type { components } from '../api/schema'
import { Button, Select } from '../primitives'
import { MailListsEditor } from '../components/mail/JunkPolicy'

type Entry = components['schemas']['JunkMessage']
type Sensitivity = components['schemas']['OrgSensitivity']

export function Junk({ api }: { api: ApiClient }) {
  const client = useQueryClient()
  const [sensitivity, setSensitivity] = useState<Sensitivity>()
  const [message, setMessage] = useState<{ subject: string; text: string } | null>(null)
  const policy = useQuery({ queryKey: ['org-junk-policy'], queryFn: async () => {
    const result = await api.GET('/api/v1/administration/mail/junk-policy')
    if (result.error) throw result.error
    return result.data
  } })
  const quarantine = useQuery({ queryKey: ['mail-quarantine'], queryFn: async () => {
    const result = await api.GET('/api/v1/administration/mail/quarantine')
    if (result.error) throw result.error
    return result.data
  } })
  const save = useMutation({ mutationFn: async (lists: components['schemas']['MailLists']) => {
    const result = await api.PUT('/api/v1/administration/mail/junk-policy', { body: { lists, sensitivity: sensitivity ?? policy.data!.sensitivity } })
    if (result.error) throw result.error
  }, onSuccess: () => client.invalidateQueries({ queryKey: ['org-junk-policy'] }) })
  const read = useMutation({ mutationFn: async (entry: Entry) => {
    const result = await api.POST('/api/v1/administration/mail/quarantine/read', { body: { workspace_id: entry.workspace_id, mailbox: entry.mailbox, message_key: entry.message_key } })
    if (result.error) throw result.error
    return result.data
  }, onSuccess: (data, entry) => setMessage({ subject: entry.subject, text: data!.text }) })
  const release = useMutation({ mutationFn: async (entry: Entry) => {
    const result = await api.POST('/api/v1/administration/mail/quarantine/release', { body: { workspace_id: entry.workspace_id, mailbox: entry.mailbox, message_key: entry.message_key } })
    if (result.error) throw result.error
  }, onSuccess: () => { setMessage(null); return client.invalidateQueries({ queryKey: ['mail-quarantine'] }) } })
  return <section>
    <h2>Junk</h2>
    {policy.isError && <p role="alert">Could not read Junk settings.</p>}
    {policy.data && <MailListsEditor key={JSON.stringify(policy.data)} lists={policy.data.lists} onSave={(lists) => save.mutate(lists)} pending={save.isPending} failed={save.isError}>
      <Select label="Org sensitivity" value={sensitivity ?? policy.data.sensitivity} onValueChange={(value) => setSensitivity(value as Sensitivity)} items={[
        { value: 'low', label: 'Low' }, { value: 'normal', label: 'Normal' }, { value: 'high', label: 'High' },
      ]} />
    </MailListsEditor>}
    <h3>Quarantine</h3>
    <p>Only Administrators read and release quarantined messages. A release writes an Org Audit Log entry.</p>
    {(quarantine.isError || read.isError || release.isError) && <p role="alert">Could not complete the quarantine request.</p>}
    {quarantine.data?.items.length === 0 && <p>Quarantine is empty.</p>}
    {quarantine.data?.items.map((entry) => <article key={JSON.stringify([entry.workspace_id, entry.mailbox, entry.message_key])}>
      <h4>{entry.subject}</h4><p>{entry.from} to {entry.mailbox}</p>
      <p>{entry.reasons.join(', ')}</p>
      <Button disabled={read.isPending} onClick={() => read.mutate(entry)}>Read {entry.subject}</Button>
      <Button disabled={release.isPending} onClick={() => release.mutate(entry)}>Release {entry.subject}</Button>
    </article>)}
    {message && <section aria-label="Quarantined message"><h4>{message.subject}</h4><pre>{message.text}</pre></section>}
  </section>
}
