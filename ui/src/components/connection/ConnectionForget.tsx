// The Forget section of a connection page: one row that ends
// everything this account taught Pagis. The preview names what goes
// before anything is removed.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Frame, Row } from '../../primitives'
import { errorMessage } from '../../queries'
import { ForgetControl } from '../ForgetControl'

import './connection.css'

export function ConnectionForget({
  api,
  connectionId,
}: {
  api: ApiClient
  connectionId: string
}) {
  const client = useQueryClient()
  const [startedId, setStartedId] = useState<string>()
  const target = { kind: 'account' as const, connection_id: connectionId }
  const operations = useQuery({
    queryKey: ['forget-operations'],
    queryFn: async () => {
      const response = await api.GET('/api/v1/knowledge/forget')
      if (response.error) throw response.error
      return response.data
    },
    refetchInterval: (query) =>
      query.state.data?.some(
        (operation) => operation.phase !== 'complete' && !operation.error,
      )
        ? 500
        : false,
  })

  async function refresh() {
    await client.invalidateQueries({ queryKey: ['forget-operations'] })
    await client.invalidateQueries({ queryKey: ['account-sync', connectionId] })
    await client.invalidateQueries({ queryKey: ['memory'] })
  }
  const retry = async (id: string) => {
    const response = await api.POST('/api/v1/knowledge/forget/{id}/retry', {
      params: { path: { id } },
    })
    if (response.error) throw response.error
    await refresh()
  }
  const reopt = async (id: string) => {
    const response = await api.POST('/api/v1/knowledge/forget/{id}/reopt', {
      params: { path: { id } },
    })
    if (response.error) throw response.error
    await refresh()
  }

  return (
    <Frame>
      <Row>
        <span className="connection-forget-copy">
          <span>Forget everything imported from this account</span>
          <span className="connection-hint">
            Removes the pages and the timeline entries this account wrote, and
            blocks a reimport. Disconnecting alone does not.
          </span>
        </span>
        <span className="connection-row-spacer" />
        <ForgetControl
          label="everything imported from this account"
          operation={operations.data?.find((operation) => operation.id === startedId)}
          preview={async () => {
            const response = await api.POST('/api/v1/knowledge/forget/preview', {
              body: { target },
            })
            if (response.error) throw response.error
            return response.data
          }}
          confirm={async (preview) => {
            const response = await api.POST('/api/v1/knowledge/forget', {
              body: { target, preview },
            })
            if (response.error) throw response.error
            setStartedId(response.data.id)
            await refresh()
            return response.data
          }}
          retry={retry}
          reopt={reopt}
        />
      </Row>
      {(operations.data ?? [])
        .filter(
          (operation) =>
            operation.connection_id === connectionId && operation.id !== startedId,
        )
        .map((operation) => (
          <Row key={operation.id}>
            <ForgetControl
              label={`the operation from ${new Date(operation.created_at).toLocaleString()}`}
              operation={operation}
              preview={async () => {
                throw new Error('A finished preview cannot be used again')
              }}
              confirm={async () => {
                throw new Error('Start from a fresh preview')
              }}
              retry={retry}
              reopt={reopt}
            />
          </Row>
        ))}
      {operations.isError && (
        <Row>
          <p className="connection-error" role="alert">
            {errorMessage(operations.error, 'The deletion state could not be read.')}
          </p>
        </Row>
      )}
    </Frame>
  )
}
