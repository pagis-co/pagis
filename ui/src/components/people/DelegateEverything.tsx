import type { ApiClient } from '../../api/client'
import { useWorkspaceAccess } from '../../workspace'
import { errorMessage, useSetDelegateEverything, useWorkspace } from '../../queries'

export function DelegateEverything({ api }: { api: ApiClient }) {
  const { isPrivate } = useWorkspaceAccess()
  const workspace = useWorkspace(api)
  const save = useSetDelegateEverything(api)
  if (!isPrivate) return null
  if (!workspace.data) return workspace.isError ? <p role="alert">Cannot read Delegate everything.</p> : null
  return <div>
    <label><input type="checkbox" checked={workspace.data.delegate_everything}
      disabled={save.isPending} onChange={event => save.mutate(event.target.checked)} /> Delegate everything</label>
    <p>Your Chief of Staff handles mail and answers colleagues within your rules, even while you are active.</p>
    {save.isError && <p role="alert">{errorMessage(save.error, 'Cannot save Delegate everything.')}</p>}
  </div>
}
