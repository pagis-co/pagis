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
