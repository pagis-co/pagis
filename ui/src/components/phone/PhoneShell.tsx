import { Capacitor } from '@capacitor/core'
import { Outlet, useMatches, useNavigate, useSearch } from '@tanstack/react-router'
import { useRef, type ReactNode } from 'react'
import type { ApiClient } from '../../api/client'
import { TabBar } from './TabBar'
import { ApprovalSheet } from './ApprovalSheet'
import { useMailInspector } from '../../state/stores'
import { MailInspector } from '../MailInspector'
import { Sheet } from '../../primitives'
import { usePhoneBack } from '../../mobileShell'
import { useEdgeSwipeBack } from '../../swipeBack'
import './phone.css'

export function PhoneShell({ api, banner }: { api: ApiClient; banner: ReactNode }) {
  const matches = useMatches()
  const search = useSearch({ strict: false }) as { request?: string }
  const navigate = useNavigate()
  const mail = useMailInspector((state) => state.mail)
  const closeMail = useMailInspector((state) => state.close)
  const mode = [...matches].reverse().find((match) => match.staticData.phone)?.staticData.phone
  const main = useRef<HTMLElement>(null)
  // A pushed screen of the iOS Mobile App goes back on a swipe from the
  // left edge. A phone browser and Android have their own back gesture.
  useEdgeSwipeBack(main, usePhoneBack(), mode === 'pushed' && Capacitor.getPlatform() === 'ios')
  return (
    <div className="phone-shell">
      {banner}
      <main ref={main} className="phone-main">
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
