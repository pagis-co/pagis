import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState, type FormEvent, type ReactNode } from 'react'
import type { ApiClient } from '../../api/client'
import type { components } from '../../api/schema'
import { Button, Textarea } from '../../primitives'
import { useCurrentWorkspaceId } from '../../workspace'
import { workspaceMailKey } from '../../queries'

type Lists = components['schemas']['MailLists']

export function MailListsEditor({ lists, children, onSave, pending, failed }: {
  lists: Lists
  children?: ReactNode
  onSave: (lists: Lists) => void
  pending: boolean
  failed: boolean
}) {
  const [allow, setAllow] = useState(lists.allow.join('\n'))
  const [block, setBlock] = useState(lists.block.join('\n'))
  const entries = (text: string) => text.split('\n').map((entry) => entry.trim()).filter(Boolean)
  function submit(event: FormEvent) {
    event.preventDefault()
    onSave({ allow: entries(allow), block: entries(block) })
  }
  return <form onSubmit={submit}>
    {children}
    <p>Use one address or domain on each line. The Org block list takes priority, then the Org allow list, then the Workspace lists.</p>
    <label>Allow list<Textarea value={allow} onChange={(event) => setAllow(event.target.value)} /></label>
    <label>Block list<Textarea value={block} onChange={(event) => setBlock(event.target.value)} /></label>
    <Button type="submit" disabled={pending}>Save Junk settings</Button>
    {failed && <p role="alert">Could not save Junk settings.</p>}
  </form>
}

export function WorkspaceJunkPolicy({ api }: { api: ApiClient }) {
  const workspace = useCurrentWorkspaceId()
  const client = useQueryClient()
  const key = [...workspaceMailKey(workspace), 'junk-policy']
  const policy = useQuery({ queryKey: key, queryFn: async () => {
    const result = await api.GET('/api/v1/workspace/mail/junk-policy')
    if (result.error) throw result.error
    return result.data
  } })
  const [mode, setMode] = useState<components['schemas']['MailScoringMode']>()
  const save = useMutation({ mutationFn: async (lists: Lists) => {
    const result = await api.PUT('/api/v1/workspace/mail/junk-policy', { body: { lists, spam_mode: mode ?? policy.data!.spam_mode } })
    if (result.error) throw result.error
  }, onSuccess: () => client.invalidateQueries({ queryKey: workspaceMailKey(workspace) }) })
  return <section>
    <h4>Junk</h4>
    {policy.isError && <p role="alert">Could not read Junk settings.</p>}
    {policy.data && <MailListsEditor key={JSON.stringify(policy.data)} lists={policy.data.lists} onSave={(lists) => save.mutate(lists)} pending={save.isPending} failed={save.isError}>
      <label><input type="checkbox" checked={(mode ?? policy.data.spam_mode) === 'move_to_junk'} onChange={(event) => setMode(event.target.checked ? 'move_to_junk' : 'score_only')} />Move junk to Junk</label>
      <p>Score-only mode records verdicts and moves nothing. Spam and Not spam send your corrections to the mail host.</p>
    </MailListsEditor>}
  </section>
}
