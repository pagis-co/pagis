// What the product says where a person needs a provider the
// installation has not set up, or where the installation's setup needs
// repair.
//
// The carrier account, its SIP sign-in, the mail domain and the Google
// OAuth client are the installation's, and the Administration Interface
// is the one place that sets them up. So the product states what is
// missing and who fixes it. An administrator also gets the link to the
// Providers view of the Administration Interface; a member gets no link,
// because that port refuses them.

import type { ReactNode } from 'react'

import type { ApiClient } from '../../api/client'
import { useUser } from '../../queries'

export function AdministratorSetsUp({
  api,
  children,
  tone = 'hint',
  testId,
}: {
  api: ApiClient
  /** What is missing, in the words of the page it shows on. */
  children: ReactNode
  tone?: 'hint' | 'warning'
  testId?: string
}) {
  const address = useUser(api).data?.administration ?? null

  return (
    <p
      className={tone === 'warning' ? 'settings-warning' : 'settings-hint'}
      data-testid={testId}
    >
      {children} An administrator sets it up in the Administration Interface.
      {address !== null && (
        <>
          {' '}
          <a href={`${address.origin}/providers`} target="_blank" rel="noreferrer">
            Open the Administration Interface
          </a>
        </>
      )}
    </p>
  )
}
