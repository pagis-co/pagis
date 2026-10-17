import { render, screen, fireEvent } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import { ConversationComposer } from './ConversationComposer'

it('sends the person-only mark and clears it with the message', () => {
  const send = vi.fn()
  render(<ConversationComposer onSend={send} />)
  fireEvent.click(screen.getByRole('checkbox', { name: 'For the person only' }))
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'Please review' } })
  fireEvent.click(screen.getByRole('button', { name: 'Send' }))
  expect(send).toHaveBeenCalledWith('Please review', true)
  expect((screen.getByRole('checkbox', { name: 'For the person only' }) as HTMLInputElement).checked).toBe(false)
})

it('offers participant names in the existing mention syntax', () => {
  const send = vi.fn()
  render(<ConversationComposer onSend={send} participants={[{ person_id: 'alex', workspace_id: 'office', name: 'Alex', presence: 'away' }]} />)
  fireEvent.click(screen.getByRole('button', { name: 'Mention Alex' }))
  expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('@Alex ')
  expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Message' }))
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: '@Alex please help' } })
  fireEvent.click(screen.getByRole('button', { name: 'Send' }))
  expect(send).toHaveBeenCalledWith('@Alex please help', false)
})

it('shows the reply target and lets the Person cancel it', () => {
  const cancel = vi.fn()
  render(<ConversationComposer onSend={vi.fn()} replyTo="Alex" onCancelReply={cancel} />)
  expect(screen.getByText('Reply to Alex')).toBeTruthy()
  fireEvent.click(screen.getByRole('button', { name: 'Cancel reply' }))
  expect(cancel).toHaveBeenCalledOnce()
})
