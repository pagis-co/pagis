import { useState } from 'react'
import type { components } from '../api/schema'
import { Button } from '../primitives'
import { errorMessage } from '../queries'

type Preview = components['schemas']['ForgetPreview']
type Operation = components['schemas']['ForgetOperation']

export function ForgetControl({ label, preview, confirm, retry, reopt, operation }: {
  label: string
  preview: () => Promise<Preview>
  confirm: (preview: Preview) => Promise<Operation>
  retry: (id: string) => Promise<unknown>
  reopt: (id: string) => Promise<unknown>
  operation?: Operation
}) {
  const [review, setReview] = useState<Preview>()
  const [started, setStarted] = useState<Operation>()
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<unknown>()
  const current = operation ?? started
  async function perform(action: () => Promise<unknown>) {
    setPending(true)
    setError(undefined)
    try { await action() } catch (failure) { setError(failure) }
    finally { setPending(false) }
  }
  return <section aria-label={`Forget ${label}`}>
    {!current && !review && <Button variant="danger-quiet" disabled={pending}
      onClick={() => void perform(async () => setReview(await preview()))}>Preview what goes</Button>}
    {!current && review && <>
      <p>This removes {review.source_items} imported source items.</p>
      <p>Note content from {review.memory_paths} paths across {review.memory_revisions} Git revisions will be purged. Unrelated notes are retained.</p>
      <p>All versions of the selected source items and their dependent page content are included.</p>
      {review.raw_account_retrieval_blocked && <p>Raw account retrieval stays blocked while suppression remains active.</p>}
      <p>External providers, provider logs, user backups and already delivered messages use their separate deletion controls.</p>
      <p>Forgetting also prevents automatic reimport. Disconnecting or pausing acquisition does not perform this deletion.</p>
      <Button disabled={pending} onClick={() => void perform(async () => setStarted(await confirm(review)))}>Confirm forget</Button>
      <Button disabled={pending} onClick={() => setReview(undefined)}>Cancel</Button>
    </>}
    {current && current.phase !== 'complete' && !current.error && <p role="status">Blocked now. Deletion is pending; suppression remains active until the purge finishes and afterward.</p>}
    {current?.error && <>
      <p role="alert">Deletion failed. The information is still blocked. {current.error}</p>
      <Button disabled={pending} onClick={() => void perform(() => retry(current.id))}>Retry deletion</Button>
    </>}
    {current?.phase === 'complete' && current.reopted_at == null && <>
      <p role="status">Deletion completed. Suppression remains active, including after reconnecting this account.</p>
      <p>Allowing acquisition again permits a fresh import. It does not restore deleted pages or their old revisions.</p>
      <Button disabled={pending} onClick={() => void perform(() => reopt(current.id))}>Allow learning again</Button>
    </>}
    {current?.reopted_at != null && <p role="status">Learning was explicitly allowed again.</p>}
    {error != null && <p role="alert">{errorMessage(error, 'Could not update forget controls. Review the current state and try again.')}</p>}
  </section>
}
