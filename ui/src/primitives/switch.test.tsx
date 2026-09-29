// The switch is one button with the switch role: a press flips it, and
// its state is in aria-checked.

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { Switch } from './switch'

describe('Switch', () => {
  it('shows its state and asks for the other state on a press', async () => {
    const user = userEvent.setup()
    const change = vi.fn()
    render(<Switch checked={false} onCheckedChange={change}>Play sound cues</Switch>)

    const control = screen.getByRole('switch', { name: 'Play sound cues' })
    expect(control.getAttribute('aria-checked')).toBe('false')

    await user.click(control)
    expect(change).toHaveBeenCalledWith(true)
  })

  it('reads on when it is checked', () => {
    render(<Switch checked onCheckedChange={() => {}}>Play sound cues</Switch>)
    expect(screen.getByRole('switch').getAttribute('aria-checked')).toBe('true')
  })
})
