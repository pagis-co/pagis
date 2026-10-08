import { useRef, useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { PhoneHeaderAction } from './TopBar'
import { Plus } from 'lucide-react'
import type { ApiClient } from '../../api/client'
import type { components } from '../../api/schema'
import {
  Badge,
  Button,
  Frame,
  IconButton,
  Input,
  Row,
  SectionLabel,
  Select,
  Sheet,
} from '../../primitives'
import { useAddTrustEntry, useDeleteTrustEntry, useTrustList } from '../../queries'
import { formatE164 } from '../../blocks/call'

function Contact({ api, row }: { api: ApiClient; row: components['schemas']['TrustEntryDto'] }) {
  const remove = useDeleteTrustEntry(api)
  const start = useRef<number | null>(null)
  const [show, setShow] = useState(false)
  return (
    <div
      className="phone-swipe"
      onPointerDown={(event) => {
        start.current = event.clientX
      }}
      onPointerUp={(event) => {
        if (start.current !== null && start.current - event.clientX > 60) setShow(true)
        start.current = null
      }}
    >
      <Row onClick={() => setShow(!show)}>
        <span className="phone-row-copy">
          <span>{row.label || 'No label'}</span>
          <span className="phone-hint">
            {row.subject === 'number'
              ? formatE164(row.value)
              : row.subject === 'domain'
                ? `@${row.value}`
                : row.value}
          </span>
        </span>
        <Badge tone={row.tier === 'owner' ? 'accent' : 'neutral'}>
          {row.tier === 'owner' ? 'Owner' : 'Trusted'}
        </Badge>
      </Row>
      {show && (
        <Button
          variant="danger"
          disabled={remove.isPending}
          aria-label={`Remove ${row.value}`}
          onClick={() => remove.mutate(row.id)}
        >
          Remove
        </Button>
      )}
      {remove.isError && (
        <p role="alert" className="phone-hint">
          {remove.error.message}
        </p>
      )}
    </div>
  )
}

export function TrustedContactsPhone({ api }: { api: ApiClient }) {
  const navigate = useNavigate()
  const list = useTrustList(api)
  const add = useAddTrustEntry(api)
  const [open, setOpen] = useState(false)
  const [label, setLabel] = useState('')
  const [value, setValue] = useState('')
  const [tier, setTier] = useState('trusted')
  return (
    <>
      <PhoneHeaderAction>
        <IconButton
          icon={Plus}
          label="Add a trusted contact"
          variant="link"
          onClick={() => setOpen(true)}
        />
      </PhoneHeaderAction>
      <h1 className="phone-heading">Trusted contacts</h1>
      <p className="phone-hint">
        What the words of a caller or a sender are worth. Never whether Pagis answers.
      </p>
      <Frame>
        <Row
          chevron
          onClick={() => void navigate({ to: '/settings/trusted-contacts/keypad' })}
          hint={list.data?.keypad_code.configured ? 'A code is set.' : 'No code is set.'}
        >
          Keypad code
        </Row>
      </Frame>
      {[
        { title: 'Phone numbers', subjects: ['number'] },
        { title: 'Email senders', subjects: ['address', 'domain'] },
      ].map((group) => (
        <section className="phone-section" key={group.title}>
          <SectionLabel>{group.title}</SectionLabel>
          <Frame>
            {group.title === 'Email senders' &&
              list.data?.own_addresses.map((row) => (
                <Row key={row.address}>
                  <span className="phone-row-copy">
                    <span>Me</span>
                    <span className="phone-hint">{row.address}</span>
                  </span>
                  <Badge tone="accent">Owner</Badge>
                </Row>
              ))}
            {list.data?.items
              .filter((row) => group.subjects.includes(row.subject))
              .map((row) => (
                <Contact key={row.id} api={api} row={row} />
              ))}
          </Frame>
        </section>
      ))}
      <p className="phone-hint">
        <strong>Owner</strong> speaks as you. <strong>Trusted</strong> is believed but cannot
        approve. <strong>Unknown</strong> is read as foreign text. Tiers apply everywhere in the
        workspace.
      </p>
      {list.isError && (
        <p role="alert" className="phone-hint">
          Could not read trusted contacts.
        </p>
      )}
      <Sheet
        open={open}
        onOpenChange={setOpen}
        title="Add a trusted contact"
        action={{
          label: 'Add',
          disabled: add.isPending || !value.trim(),
          onSelect: () =>
            add.mutate(
              { label: label.trim(), value: value.trim(), tier },
              {
                onSuccess: () => {
                  setOpen(false)
                  setLabel('')
                  setValue('')
                },
              },
            ),
        }}
      >
        <div className="phone-form">
          <label className="phone-form-field">
            Label
            <Input value={label} onChange={(event) => setLabel(event.target.value)} />
          </label>
          <label className="phone-form-field">
            Phone number, email address or domain
            <Input value={value} onChange={(event) => setValue(event.target.value)} />
          </label>
          <Select
            label="Tier"
            value={tier}
            onValueChange={setTier}
            items={[
              { value: 'trusted', label: 'Trusted' },
              { value: 'owner', label: 'Owner' },
            ]}
          />
          {add.isError && (
            <p role="alert" className="phone-hint">
              {add.error.message}
            </p>
          )}
        </div>
      </Sheet>
    </>
  )
}
