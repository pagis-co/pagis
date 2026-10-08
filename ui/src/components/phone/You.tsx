import { useEffect, useState } from 'react'
import { Capacitor } from '@capacitor/core'
import { useNavigate } from '@tanstack/react-router'
import { BookOpen, Package, Repeat, SlidersHorizontal } from 'lucide-react'
import type { ApiClient } from '../../api/client'
import {
  ActionSheet,
  Button,
  Frame,
  OwnerAvatar,
  Row,
  SectionLabel,
  Switch,
} from '../../primitives'
import { useSchedules, useSignOut, useUserName } from '../../queries'
import { PagisShell } from '../../mobileShell'
import { NotificationSwitch } from '../settings/Notifications'

export function You({ api }: { api: ApiClient }) {
  const name = useUserName(api)
  const navigate = useNavigate()
  const schedules = useSchedules(api)
  const signOut = useSignOut(api)
  const [confirm, setConfirm] = useState<'server' | 'out' | null>(null)
  const [answers, setAnswers] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)
  const native = Capacitor.isNativePlatform()
  useEffect(() => {
    if (native)
      void PagisShell.getLockScreenAnswers()
        .then(({ on }) => setAnswers(on))
        .catch(() => setFailure('The lock-screen setting could not be read.'))
  }, [native])
  const next = (schedules.data ?? [])
    .filter((row) => row.next_due_at != null)
    .sort((a, b) => (a.next_due_at ?? 0) - (b.next_due_at ?? 0))[0]
  const places = [
    { title: 'Memory', hint: 'What your sprites know', icon: BookOpen, to: '/memory' },
    {
      title: 'Automations',
      hint: next
        ? `Next: ${next.name}, ${new Date(next.next_due_at!).toLocaleString([], { weekday: 'short', hour: '2-digit', minute: '2-digit' })}`
        : 'No Schedule yet.',
      icon: Repeat,
      to: '/automations',
    },
    {
      title: 'Software',
      hint: 'Small programs your sprites wrote',
      icon: Package,
      to: '/software',
    },
    {
      title: 'Settings',
      hint: 'Connections, sessions and the vault',
      icon: SlidersHorizontal,
      to: '/settings',
    },
  ] as const
  return (
    <div className="you-phone">
      <header className="you-phone-head">
        <OwnerAvatar name={name} size="xl" outlined />
        <div>
          <h1 className="phone-heading">{name}</h1>
          <p className="phone-hint you-phone-address">{window.location.host}</p>
        </div>
      </header>
      <Frame>
        {places.map(({ title, hint, icon: Icon, to }) => (
          <Row key={to} chevron onClick={() => void navigate({ to })}>
            <Icon size={20} aria-hidden />
            <span className="phone-row-copy">
              <span>{title}</span>
              <span className="phone-hint">{hint}</span>
            </span>
          </Row>
        ))}
      </Frame>
      <section className="phone-section">
        <SectionLabel>This phone</SectionLabel>
        <Frame>
          <NotificationSwitch api={api} />
          {native && (
            <Switch
              row
              checked={answers}
              onCheckedChange={(on) => {
                void PagisShell.setLockScreenAnswers({ on })
                  .then(() => setAnswers(on))
                  .catch(() => setFailure('The lock-screen setting could not be saved.'))
              }}
            >
              <span className="phone-row-copy">
                <span>Answer on the lock screen</span>
                <span className="phone-hint">Approve once or deny from a notification.</span>
              </span>
            </Switch>
          )}
          <div className="you-phone-session-actions">
            {native && (
              <Button variant="link" onClick={() => setConfirm('server')}>
                Change server
              </Button>
            )}
            <Button variant="link" onClick={() => setConfirm('out')}>
              Sign out
            </Button>
          </div>
        </Frame>
      </section>
      {(failure || signOut.isError) && <p role="alert">{failure ?? signOut.error?.message}</p>}
      <ActionSheet
        open={confirm !== null}
        onOpenChange={(open) => {
          if (!open) setConfirm(null)
        }}
        title={confirm === 'server' ? 'Change server?' : 'Sign out of Pagis?'}
        description={
          confirm === 'server' ? (
            <>
              This phone forgets {window.location.host} and opens the Connect screen.
              <br />
              <br />
              Your sprites keep working. The sign-in of this phone stays in Settings › Sessions
              until you revoke it there.
            </>
          ) : (
            <>
              This phone stops getting notifications. Your sprites keep working.
              <br />
              <br />
              To sign in again, scan a new sign-in link.
            </>
          )
        }
        action={{
          label: confirm === 'server' ? 'Change server' : 'Sign out',
          danger: confirm !== 'server',
          disabled: signOut.isPending,
          onSelect: () => {
            if (confirm === 'server')
              void PagisShell.changeServer().catch(() =>
                setFailure('The server could not be changed.'),
              )
            else signOut.mutate()
            setConfirm(null)
          },
        }}
      />
    </div>
  )
}
