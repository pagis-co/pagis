import { useState } from 'react'

import type { ApiClient, CredentialDto } from '../../api/client'
import { Button, Dialog, Input, Sheet } from '../../primitives'
import { useIsMobile } from '../../state/useIsMobile'
import { errorMessage, useAddCredential, useDeleteCredential } from '../../queries'

import './SignInDialog.css'

/** The form that saves a sign-in. With `replacing` it
 * starts from that sign-in's site and login, saves the new record
 * first, and deletes the old one only once the new one is in. The
 * password goes one way: the field starts empty every time. */
export function SignInDialog({
  api,
  open,
  onOpenChange,
  replacing,
}: {
  api: ApiClient
  open: boolean
  onOpenChange: (open: boolean) => void
  replacing?: CredentialDto
}) {
  const Modal = useIsMobile() ? Sheet : Dialog
  const add = useAddCredential(api)
  const remove = useDeleteCredential(api)
  const [domain, setDomain] = useState(replacing?.domain ?? '')
  const [username, setUsername] = useState(replacing?.username ?? '')
  const [loginUrl, setLoginUrl] = useState(replacing?.login_url ?? '')
  const [secret, setSecret] = useState('')
  const [totpSeed, setTotpSeed] = useState('')

  const ready =
    domain.trim() !== '' && username.trim() !== '' && loginUrl.trim() !== '' && secret !== ''
  const pending = add.isPending || remove.isPending

  const save = async () => {
    await add.mutateAsync({
      domain: domain.trim(),
      username: username.trim(),
      login_url: loginUrl.trim(),
      secret,
      totp_seed: totpSeed.trim() === '' ? undefined : totpSeed.trim(),
    })
    if (replacing !== undefined) await remove.mutateAsync(replacing.id)
    onOpenChange(false)
  }

  return (
    <Modal
      open={open}
      onOpenChange={onOpenChange}
      title={replacing === undefined ? 'Add a sign-in' : `Replace the sign-in for ${replacing.domain}`}
      description="A sprite fills this on the site's login page. It never reads the password."
      footer={
        <Button
          variant="primary"
          disabled={!ready || pending}
          onClick={() => void save().catch(() => undefined)}
        >
          Save the sign-in
        </Button>
      }
    >
      <form className="sign-in-form" onSubmit={(event) => event.preventDefault()}>
        <label className="sign-in-field">
          <span>Site</span>
          <Input
            placeholder="example.com"
            value={domain}
            onChange={(event) => setDomain(event.target.value)}
          />
        </label>
        <label className="sign-in-field">
          <span>Username or email</span>
          <Input value={username} onChange={(event) => setUsername(event.target.value)} />
        </label>
        <label className="sign-in-field">
          <span>Sign-in address</span>
          <Input
            placeholder="https://example.com/login"
            value={loginUrl}
            onChange={(event) => setLoginUrl(event.target.value)}
          />
        </label>
        <label className="sign-in-field">
          <span>Password</span>
          <Input
            type="password"
            autoComplete="new-password"
            value={secret}
            onChange={(event) => setSecret(event.target.value)}
          />
        </label>
        <label className="sign-in-field">
          <span>One-time code seed</span>
          <Input
            type="password"
            autoComplete="off"
            placeholder="Optional"
            value={totpSeed}
            onChange={(event) => setTotpSeed(event.target.value)}
          />
        </label>
        {add.isError && (
          <p className="settings-error" role="alert">
            {errorMessage(add.error, 'That sign-in could not be saved.')}
          </p>
        )}
        {remove.isError && (
          <p className="settings-error" role="alert">
            The new sign-in is saved. The old one could not be deleted; delete it from the list.
          </p>
        )}
      </form>
    </Modal>
  )
}
