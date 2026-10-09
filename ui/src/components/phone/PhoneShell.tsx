import { Outlet, useMatches, useNavigate, useSearch } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import type { ApiClient } from '../../api/client'
import { TabBar } from './TabBar'
import { ApprovalSheet } from './ApprovalSheet'
import { useMailInspector } from '../../state/stores'
import { MailInspector } from '../MailInspector'
import { Sheet } from '../../primitives'
import './phone.css'

export function PhoneShell({ api, banner }: { api: ApiClient; banner: ReactNode }) {
  const matches = useMatches()
  const search = useSearch({ strict: false }) as { request?: string }
  const navigate = useNavigate()
  const mail = useMailInspector((state) => state.mail)
  const closeMail = useMailInspector((state) => state.close)
  const mode = [...matches].reverse().find((match) => match.staticData.phone)?.staticData.phone
  return (
    <div className="phone-shell">
      {banner}
      <main className="phone-main">
        <Outlet />
      </main>
      {mode === 'tab' && <TabBar api={api} />}
      {search.request && (
        <ApprovalSheet
          key={search.request}
          api={api}
          requestId={search.request}
          onClose={() =>
            void navigate({ to: '.', search: (previous) => ({ ...previous, request: undefined }) })
          }
        />
      )}
      {mail && (
        <Sheet
          open
          onOpenChange={(open) => {
            if (!open) closeMail()
          }}
          title="Mail"
        >
          <MailInspector api={api} mail={mail} onClose={closeMail} />
        </Sheet>
      )}
    </div>
  )
}
