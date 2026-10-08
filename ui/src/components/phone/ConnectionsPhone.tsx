import { useEffect, useState } from 'react'
import { Browser } from '@capacitor/browser'
import { Info, Plus } from 'lucide-react'
import { Capacitor } from '@capacitor/core'
import type { ApiClient } from '../../api/client'
import { Badge, Button, Frame, Row, Sheet } from '../../primitives'
import {
  useAuthorizeConnection,
  useConnectionProviders,
  useConnections,
  useCreateConnection,
} from '../../queries'
import { CONNECTION_STATE } from '../ConnectionStatus'
import { READ_ONLY_CONNECTION_CAPABILITIES } from '../ConnectionCapabilities'

function AddConnection({ api, onClose }: { api: ApiClient; onClose: () => void }) {
  const providers = useConnectionProviders(api)
  const connections = useConnections(api)
  const create = useCreateConnection(api)
  const authorize = useAuthorizeConnection(api)
  const [id, setId] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const entry = providers.data?.find((row) => row.id === 'google')
  const ready = entry?.set_up === true
  const connected =
    !!id && connections.data?.some((row) => row.id === id && row.status === 'connected')
  const refetch = connections.refetch
  useEffect(() => {
    if (!id) return
    const timer = window.setInterval(() => void refetch(), 1000)
    return () => clearInterval(timer)
  }, [id, refetch])
  useEffect(() => {
    if (connected) {
      if (Capacitor.isNativePlatform()) void Browser.close()
      onClose()
    }
  }, [connected, onClose])
  const start = async () => {
    setFailure(null)
    try {
      const connection = id
        ? { id }
        : await create.mutateAsync({
            provider: 'google',
            alias: `google-${crypto.randomUUID()}`,
            display_name: 'Google',
            fields: {},
          })
      setId(connection.id)
      const answer = await authorize.mutateAsync({
        connectionId: connection.id,
        capabilities: READ_ONLY_CONNECTION_CAPABILITIES,
      })
      if (answer.authorization_url) {
        if (Capacitor.isNativePlatform()) await Browser.open({ url: answer.authorization_url })
        else window.open(answer.authorization_url, '_blank', 'noopener')
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : 'Google did not complete the connection.')
    }
  }
  return (
    <Sheet
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
      title="Add a connection"
    >
      <div className="phone-form phone-add-connection">
        <header className="phone-portrait-head">
          <span className="phone-initial-tile phone-connection-mark">G</span>
          <h1 className="phone-heading">Connect a Google account</h1>
        </header>
        {providers.isPending ? (
          <p className="phone-hint">Reading the server setup…</p>
        ) : providers.isError ? (
          <p role="alert" className="phone-hint">
            Could not read the server setup.
          </p>
        ) : ready ? (
          <>
            <p className="phone-add-connection-lead">
              Sign in at Google and allow Pagis. Then you choose which sprites may use the account.
            </p>
            <Frame>
              {[
                Capacitor.isNativePlatform()
                  ? 'Google opens in a secure sign-in sheet.'
                  : 'Google opens in your browser.',
                'Choose the account and allow access.',
                'You come back here, connected.',
              ].map((line, index) => (
                <Row key={line}>
                  <span className="phone-step-number">{index + 1}</span>
                  <span>{line}</span>
                </Row>
              ))}
            </Frame>
            <Button
              variant="primary"
              size="lg"
              disabled={create.isPending || authorize.isPending}
              onClick={() => void start()}
            >
              {authorize.isPending ? 'Opening Google…' : 'Continue at Google'}
            </Button>
            {id && (
              <p role="status" className="phone-hint">
                Finish signing in at Google. This sheet closes when the account connects.
              </p>
            )}
            <p className="phone-hint">
              Pagis keeps the sign-in for this account on your server, sealed with your key. Nobody
              else’s sprites can use it.
            </p>
          </>
        ) : (
          <>
            <div className="phone-well phone-info-well">
              <Info size={20} aria-hidden />
              <span className="phone-row-copy">
                <strong>Google is not set up on this server yet.</strong>
                <span className="phone-hint">
                  An administrator sets up the Google client once in the Administration Interface.
                  Then everyone connects with a Google sign-in only.
                </span>
              </span>
            </div>
            <Button size="lg" onClick={onClose}>
              Back to Connections
            </Button>
          </>
        )}
        {failure && (
          <p role="alert" className="phone-hint">
            {failure}
          </p>
        )}
      </div>
    </Sheet>
  )
}

export function ConnectionsPhone({
  api,
  onOpen,
}: {
  api: ApiClient
  onOpen: (id: string) => void
}) {
  const connections = useConnections(api)
  const [adding, setAdding] = useState(false)
  return (
    <>
      <h1 className="phone-heading">Connections</h1>
      <p className="phone-hint">
        Connect the accounts and services your sprites use. You choose which sprites have access.
      </p>
      <Frame>
        {(connections.data ?? []).map((row) => {
          const state = CONNECTION_STATE[row.status]
          return (
            <Row key={row.id} chevron onClick={() => onOpen(row.id)}>
              <span className="phone-initial-tile">
                {row.provider === 'google' ? 'G' : row.display_name.charAt(0).toUpperCase()}
              </span>
              <span className="phone-row-copy">
                <span className="phone-row-account phone-row-line">
                  {row.account ?? row.display_name}
                </span>
                <span className="phone-hint">
                  {row.provider === 'google' ? 'Google account' : `${row.provider} account`}
                </span>
              </span>
              <Badge tone={state?.tone ?? 'neutral'}>{state?.label ?? row.status}</Badge>
            </Row>
          )
        })}
      </Frame>
      <Button variant="primary" size="lg" onClick={() => setAdding(true)}>
        <Plus size={20} aria-hidden />
        Add a connection
      </Button>
      {connections.isError && (
        <p role="alert" className="phone-hint">
          Could not read the connections.
        </p>
      )}
      <p className="phone-hint">
        A sprite that needs a connection asks in its conversation. You grant it from the
        connection's page.
      </p>
      {adding && <AddConnection api={api} onClose={() => setAdding(false)} />}
    </>
  )
}
