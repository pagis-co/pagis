// The `mail` block (ADR-0019): one line for an inbound mail that
// woke the Agent and for a mail the Agent sent, and the way into the
// mail inspector.

import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import { useCallInspector, useMailInspector } from '../state/stores'
import { MailBlock } from './MailBlock'
import type { MailBlockDto } from './mail'

const inbound: MailBlockDto = {
  type: 'mail',
  direction: 'inbound',
  mailbox: 'ada@example.com',
  message_id: 'INBOX:12',
  counterpart: 'Clinic <care@clinic.test>',
  subject: 'Your appointment',
  trust_tier: 'unknown',
}

const outbound: MailBlockDto = {
  type: 'mail',
  direction: 'outbound',
  mailbox: 'ada@example.com',
  message_id: '<sent-1@example.com>',
  counterpart: 'bob@other.test',
  subject: 'Invoice 42',
  trust_tier: null,
}

beforeEach(() => {
  useMailInspector.setState({ mail: null })
  useCallInspector.setState({ callId: null })
})

describe('the mail strip', () => {
  it('is one line: the direction, the other party, the subject and the mailbox', () => {
    render(<MailBlock block={inbound} />)
    const strip = screen.getByTestId('mail-strip')
    expect(strip.textContent).toContain('Mail from Clinic <care@clinic.test>')
    expect(strip.textContent).toContain('Your appointment')
    expect(strip.textContent).toContain('ada@example.com')
  })

  it('carries the tier of the sender', () => {
    render(<MailBlock block={inbound} />)
    expect(screen.getByTestId('mail-strip').textContent).toContain('Unknown')
  })

  it('shows no tier for a mail the agent sent', () => {
    render(<MailBlock block={outbound} />)
    const strip = screen.getByTestId('mail-strip')
    expect(strip.textContent).toContain('Mail to bob@other.test')
    expect(strip.textContent).not.toContain('Unknown')
  })

  it('opens the mail inspector, and gives the slot back from the call', () => {
    useCallInspector.setState({ callId: 'call_1' })
    render(<MailBlock block={inbound} />)
    fireEvent.click(screen.getByTestId('mail-strip'))
    expect(useMailInspector.getState().mail).toEqual(inbound)
    expect(useCallInspector.getState().callId).toBeNull()
  })

  it('names a message that carries no subject', () => {
    render(<MailBlock block={{ ...inbound, subject: '' }} />)
    expect(screen.getByTestId('mail-strip').textContent).toContain('(no subject)')
  })
})
