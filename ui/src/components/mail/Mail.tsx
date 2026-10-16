import { Link, useNavigate } from '@tanstack/react-router'
import { useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Button, Input, Select, Sheet } from '../../primitives'
import { useIsMobile } from '../../primitives/useMediaQuery'
import {
  useWorkspaceMailbox,
  useWorkspaceMailFolders,
  useWorkspaceMailMessages,
  workspaceMailMessagesKey,
} from '../../queries'
import { useCurrentWorkspaceId } from '../../workspace'
import { MessageList } from './MessageList'
import { MailThread } from './MailThread'
import { Composer } from './Composer'
import { FolderManager } from './FolderManager'
import { NavBar } from '../phone/TopBar'

import './mail.css'

export function Mail({ api }: { api: ApiClient }) {
  const phone = useIsMobile()
  const navigate = useNavigate()
  const mailbox = useWorkspaceMailbox(api)
  const [messageId, setMessageId] = useState<string | null>(null)
  const [composing, setComposing] = useState(false)
  const [view, setView] = useState<'needs-you' | 'all'>('needs-you')
  const reading = phone && messageId !== null && view === 'all'

  return (
    <section className={`mail-place${phone ? ' mail-phone' : ''}`}>
      {phone && !reading && (
        <NavBar
          back={{ label: 'You', onBack: () => void navigate({ to: '/you' }) }}
          actions={mailbox.data?.mailbox && (
            <Button variant="link" onClick={() => setComposing(true)}>Compose</Button>
          )}
        />
      )}
      {!reading && (
        <header className="mail-header">
          <h2>Mail</h2>
          {mailbox.data?.mailbox && <p>{mailbox.data.mailbox.address}</p>}
        </header>
      )}
      {mailbox.isError ? (
        <p role="alert">Could not load the Workspace Mailbox.</p>
      ) : !mailbox.data ? (
        <p className="mail-empty">Loading mail…</p>
      ) : !mailbox.data.mailbox ? (
        <div className="mail-empty">
          <p>You have no Workspace Mailbox.</p>
          <Link to="/settings/$section" params={{ section: 'mailbox' }}>
            Set up your Workspace Mailbox
          </Link>
        </div>
      ) : (
        <>
          {!reading && (
            <nav className="mail-views" aria-label="Mail views">
              {!phone && <Button onClick={() => setComposing(true)}>Compose</Button>}
              <Button
                aria-pressed={view === 'needs-you'}
                onClick={() => {
                  setView('needs-you')
                  setMessageId(null)
                }}
              >
                Needs you
              </Button>
              <Button
                aria-pressed={view === 'all'}
                onClick={() => setView('all')}
              >
                All mail
              </Button>
            </nav>
          )}
          {composing && (
            <Composer api={api} onClose={() => setComposing(false)} />
          )}
          {view === 'needs-you' ? (
            <p className="mail-empty">No mail needs you.</p>
          ) : (
            <AllMail api={api} messageId={messageId} onOpen={setMessageId} />
          )}
        </>
      )}
    </section>
  )
}

function AllMail({
  api,
  messageId,
  onOpen,
}: {
  api: ApiClient
  messageId: string | null
  onOpen: (id: string | null) => void
}) {
  const workspaceId = useCurrentWorkspaceId()
  const client = useQueryClient()
  const phone = useIsMobile()
  const [managingFolders, setManagingFolders] = useState(false)
  const [folder, setFolder] = useState('')
  const [q, setQ] = useState('')
  const [cursor, setCursor] = useState<string>()
  const folders = useWorkspaceMailFolders(api)
  const messages = useWorkspaceMailMessages(api, { folder, q, cursor })

  function backToMail() {
    onOpen(null)
    void client.invalidateQueries({
      queryKey: workspaceMailMessagesKey(workspaceId),
    })
  }

  return (
    <>
      {(!phone || messageId === null) && (
        <>
          <div className="mail-controls">
            <label>
              Search mail
              <Input
                type="search"
                value={q}
                onChange={(event) => {
                  setQ(event.target.value)
                  setCursor(undefined)
                  onOpen(null)
                }}
              />
            </label>
            <Select
              label="Folder"
              value={folder ? `folder:${folder}` : 'all'}
              onValueChange={(value) => {
                setFolder(value === 'all' ? '' : value.slice(7))
                setCursor(undefined)
                onOpen(null)
              }}
              items={[
                { value: 'all', label: 'All folders' },
                ...(folders.data?.items ?? []).map((name) => ({
                  value: `folder:${name}`,
                  label: name,
                })),
              ]}
            />
          </div>
          {phone ? (
            <>
              <Button onClick={() => setManagingFolders(true)}>Folders</Button>
              <Sheet open={managingFolders} onOpenChange={setManagingFolders} title="Folders">
                <div className="mail-phone mail-folders">
                  <FolderManager api={api} folders={folders.data?.items ?? []} />
                </div>
              </Sheet>
            </>
          ) : <FolderManager api={api} folders={folders.data?.items ?? []} />}
          {folders.isError && <p role="alert">Could not load mail folders.</p>}
        </>
      )}
      <div className={`mail-panes${messageId ? ' mail-panes-open' : ''}`}>
        {(!phone || messageId === null) && (
          <MessageList
            messages={messages.data?.items}
            loading={messages.isPending}
            failed={messages.isError}
            selectedId={messageId}
            onOpen={onOpen}
            cursor={cursor}
            next={messages.data?.next}
            onPage={(page) => {
              setCursor(page)
              onOpen(null)
            }}
          />
        )}
        {messageId !== null && (
          <MailThread api={api} messageId={messageId} onBack={backToMail} />
        )}
      </div>
    </>
  )
}
