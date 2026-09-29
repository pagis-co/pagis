// A toast is announced, is reachable from the keyboard through the
// viewport and dismisses.

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { Button } from './button'
import { ToastProvider, useToast } from './toast'

function Raiser() {
  const notify = useToast()
  return (
    <Button onClick={() => notify({ title: 'The run failed', tone: 'failed' })}>
      Raise
    </Button>
  )
}

describe('Toast', () => {
  it('raises a toast from a keyboard press and dismisses it', async () => {
    const user = userEvent.setup()
    render(
      <ToastProvider>
        <Raiser />
      </ToastProvider>,
    )

    await user.tab()
    await user.keyboard('{Enter}')
    expect(await screen.findByText('The run failed')).toBeTruthy()

    await user.click(screen.getByRole('button', { name: 'Dismiss' }))
    expect(screen.queryByText('The run failed')).toBeNull()
  })

  it('fails loudly when no provider is above the caller', () => {
    const previous = console.error
    console.error = () => undefined
    try {
      expect(() => render(<Raiser />)).toThrow(/ToastProvider/)
    } finally {
      console.error = previous
    }
  })
})
