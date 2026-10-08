import { useState } from 'react'

import type { ApiClient, CredentialDto } from '../../api/client'
import { ActionSheet, Avatar, Badge, Button, Frame, IconButton, Row } from '../../primitives'
import { PhoneHeaderAction } from '../phone/TopBar'
import { Lock, Plus } from 'lucide-react'
import { useIsMobile } from '../../state/useIsMobile'
import { useAgents, useCredentials, useDeleteCredential } from '../../queries'
import { SettingsSection } from './SettingsSection'
import { SignInDialog } from './SignInDialog'

import '../settings.css'
import './Vault.css'

/** The address as the row reads it: the scheme adds nothing. */
function loginAddress(loginUrl: string): string {
  return loginUrl.replace(/^https?:\/\//, '').replace(/\/$/, '')
}

function SignInRow({
  api,
  credential,
  onReplace,
}: {
  api: ApiClient
  credential: CredentialDto
  onReplace: () => void
}) {
  const phone = useIsMobile()
  const [choosing, setChoosing] = useState(false)
  const agents = useAgents(api)
  const remove = useDeleteCredential(api)
  const owner =
    credential.owner_agent_id == null
      ? undefined
      : agents.data?.find((agent) => agent.id === credential.owner_agent_id)
  if (phone) return <><Row chevron onClick={() => setChoosing(true)}><span className="phone-initial-tile"><Lock size={20} aria-hidden /></span><span className="phone-row-copy"><span>{credential.domain} {credential.has_totp && <Badge tone="accent">2FA</Badge>}</span><span className="phone-hint">{credential.owner_agent_id == null ? 'Any sprite with a Grant' : `Owner: ${owner?.name ?? 'Sprite'}`}</span></span></Row><ActionSheet open={choosing} onOpenChange={setChoosing} title={credential.domain} description="Replace or delete this sign-in." actions={[{ label: 'Replace', onSelect: onReplace }, { label: 'Delete', danger: true, disabled: remove.isPending, onSelect: () => remove.mutate(credential.id) }]} />{remove.isError && <p role="alert" className="phone-hint">{remove.error.message}</p>}</>
  return (
    <Row>
      <span className="settings-tile" aria-hidden>
        {credential.domain.charAt(0).toUpperCase()}
      </span>
      <span className="settings-row-name">
        <strong>{credential.domain}</strong>
        <span className="settings-row-detail">login at {loginAddress(credential.login_url)}</span>
      </span>
      {credential.has_totp && <Badge tone="accent">2FA</Badge>}
      <span className="settings-row-aside">
        {credential.owner_agent_id == null ? (
          'Any sprite with a Grant'
        ) : (
          <>
            Owner
            <Avatar
              id={credential.owner_agent_id}
              name={owner?.name ?? ''}
              appearance={owner?.avatar}
              size="sm"
            />
            {owner?.name}
          </>
        )}
      </span>
      <Button
        size="sm"
        aria-label={`Replace the sign-in for ${credential.domain}`}
        onClick={onReplace}
      >
        Replace
      </Button>
      <Button
        variant="ghost"
        size="sm"
        aria-label={`Delete the sign-in for ${credential.domain}`}
        disabled={remove.isPending}
        onClick={() => remove.mutate(credential.id)}
      >
        Delete
      </Button>
    </Row>
  )
}

/** The Vault: the sign-ins an Agent can fill on a site. One row
 * per sign-in, Add a sign-in as the one primary action, and the
 * secret file hint. A sign-in is replaced by a fresh one: the vault never
 * reads a password back to edit it. */
export function Vault({ api }: { api: ApiClient }) {
  const phone = useIsMobile()
  const credentials = useCredentials(api)
  // `null` is closed, `'add'` is a new sign-in, a credential is a replacement.
  const [editing, setEditing] = useState<'add' | CredentialDto | null>(null)
  const items = credentials.data ?? []
  return (
    <SettingsSection
      title="Vault"
      lead="Sign-ins your sprites can fill on a site. They fill, they never read."
      action={
        phone ? <PhoneHeaderAction><IconButton icon={Plus} label="Add a sign-in" variant="link" onClick={() => setEditing('add')} /></PhoneHeaderAction> : <Button variant="primary" onClick={() => setEditing('add')}>
          Add a sign-in
        </Button>
      }
      hint={phone ? "A tap on a sign-in shows Replace and Delete." : (
        <>
          A sign-in opens one address, the login page, and the fill happens on the sprite&apos;s
          Desk. The password stays in the installation's sealed secret file (ADR-0013); Pagis shows it to nobody,
          the sprite included. An owner sprite holds the sign-in as its own; a sign-in with no
          owner needs a Grant.
        </>
      )}
    >
      {credentials.isSuccess && items.length === 0 ? (
        <p className="vault-empty">No sign-ins yet.</p>
      ) : (
        <Frame>
          {items.map((credential) => (
            <SignInRow
              key={credential.id}
              api={api}
              credential={credential}
              onReplace={() => setEditing(credential)}
            />
          ))}
        </Frame>
      )}
      {editing !== null && (
        <SignInDialog
          key={editing === 'add' ? 'add' : editing.id}
          api={api}
          open
          onOpenChange={(open) => {
            if (!open) setEditing(null)
          }}
          replacing={editing === 'add' ? undefined : editing}
        />
      )}
    </SettingsSection>
  )
}
