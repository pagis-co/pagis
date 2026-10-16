import { useState } from 'react'
import { type ApiClient, uploadFile } from '../../api/client'
import { Button, Input, Textarea, Sheet } from '../../primitives'
import {
  type MailSource,
  useSendWorkspaceMail,
  useWorkspaceMailComposer,
} from '../../queries'
import type { components } from '../../api/schema'
import { useIsMobile } from '../../primitives/useMediaQuery'

type Draft = components['schemas']['MailComposerDto']

export function Composer({
  api,
  source,
  onClose,
}: {
  api: ApiClient
  source?: MailSource
  onClose: () => void
}) {
  const phone = useIsMobile()
  const prepared = useWorkspaceMailComposer(api, source)
  const title = source
    ? { reply: 'Reply', 'reply-all': 'Reply all', forward: 'Forward' }[source.action]
    : 'Compose'
  const content = source && prepared.isError ? (
    <p role="alert">Could not prepare mail.</p>
  ) : source && !prepared.data ? (
    <p>Loading composer…</p>
  ) : (
    <ComposerForm api={api} source={source} initial={prepared.data} onClose={onClose} />
  )
  return phone ? (
    <Sheet
      open
      onOpenChange={(open) => { if (!open) onClose() }}
      title={title}
      showHeader={false}
    >
      <div className="mail-phone mail-compose-sheet">{content}</div>
    </Sheet>
  ) : content
}

function ComposerForm({
  api,
  source,
  initial,
  onClose,
}: {
  api: ApiClient
  source?: MailSource
  initial?: Draft
  onClose: () => void
}) {
  const [to, setTo] = useState(initial?.to.join(', ') ?? '')
  const [cc, setCc] = useState(initial?.cc.join(', ') ?? '')
  const [bcc, setBcc] = useState('')
  const [subject, setSubject] = useState(initial?.subject ?? '')
  const [body, setBody] = useState(initial?.body ?? '')
  const [files, setFiles] = useState<{ id: string; name: string }[]>([])
  const [uploading, setUploading] = useState(false)
  const [uploadError, setUploadError] = useState(false)
  const send = useSendWorkspaceMail(api, source)
  const addresses = (value: string) =>
    value
      .split(/[,;]/)
      .map((a) => a.trim())
      .filter(Boolean)
  return (
    <form
      className="mail-composer"
      aria-label="Compose mail"
      onSubmit={(event) => {
        event.preventDefault()
        send.mutate(
          {
            to: addresses(to),
            cc: addresses(cc),
            bcc: addresses(bcc),
            subject,
            body,
            files: files.map((file) => file.id),
          },
          { onSuccess: onClose },
        )
      }}
    >
      <h3>
        {source
          ? { reply: 'Reply', 'reply-all': 'Reply all', forward: 'Forward' }[
              source.action
            ]
          : 'Compose'}
      </h3>
      <label>
        To
        <Input value={to} onChange={(event) => setTo(event.target.value)} />
      </label>
      <label>
        Cc
        <Input value={cc} onChange={(event) => setCc(event.target.value)} />
      </label>
      <label>
        Bcc
        <Input value={bcc} onChange={(event) => setBcc(event.target.value)} />
      </label>
      <label>
        Subject
        <Input
          value={subject}
          onChange={(event) => setSubject(event.target.value)}
        />
      </label>
      <label>
        Body
        <Textarea
          value={body}
          onChange={(event) => setBody(event.target.value)}
          rows={10}
        />
      </label>
      {source?.action === 'forward' && (
        <p>The original attachments are included.</p>
      )}
      <label>
        Attach Files
        <Input
          type="file"
          multiple
          disabled={uploading || send.isPending}
          onChange={async (event) => {
            const selected = Array.from(event.target.files ?? [])
            event.target.value = ''
            setUploading(true)
            setUploadError(false)
            try {
              for (const file of selected) {
                const stored = await uploadFile(file)
                setFiles((previous) => [
                  ...previous,
                  { id: stored.id, name: file.name },
                ])
              }
            } catch {
              setUploadError(true)
            } finally {
              setUploading(false)
            }
          }}
        />
      </label>
      <ul>
        {files.map((file, index) => (
          <li key={`${file.id}:${index}`}>
            {file.name}
            <Button
              type="button"
              onClick={() =>
                setFiles((previous) => previous.filter((_, at) => at !== index))
              }
            >
              Remove {file.name}
            </Button>
          </li>
        ))}
      </ul>
      {uploadError && <p role="alert">Could not upload the File.</p>}
      {send.isError && (
        <p role="alert">
          Could not send mail. Check the recipients and the Outgoing Cap.
        </p>
      )}
      <div className="mail-controls">
        <Button
          type="submit"
          disabled={
            uploading ||
            send.isPending ||
            !(to.trim() || cc.trim() || bcc.trim())
          }
        >
          Send
        </Button>
        <Button type="button" disabled={send.isPending} onClick={onClose}>
          Cancel
        </Button>
      </div>
    </form>
  )
}
