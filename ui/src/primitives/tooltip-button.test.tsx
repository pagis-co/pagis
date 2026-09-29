// A button with words and a tooltip. The words name it; the tooltip
// adds a hint, such as the key that does the same act.

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { TooltipButton } from './index'

describe('TooltipButton', () => {
  it('is named by its words and presses on Enter', async () => {
    const user = userEvent.setup()
    const press = vi.fn()
    render(
      <TooltipButton tooltip="Send the message (Enter)" onClick={press}>
        Send
      </TooltipButton>,
    )

    const button = screen.getByRole('button', { name: 'Send' })
    await user.tab()
    expect(document.activeElement).toBe(button)
    await user.keyboard('{Enter}')
    expect(press).toHaveBeenCalledTimes(1)
  })

  it('shows the tooltip when the focus arrives', async () => {
    const user = userEvent.setup()
    render(<TooltipButton tooltip="Send the message (Enter)">Send</TooltipButton>)

    await user.tab()
    expect((await screen.findByRole('tooltip')).textContent).toBe('Send the message (Enter)')
  })

  it('takes the Button variant', () => {
    render(
      <TooltipButton variant="primary" tooltip="Send the message (Enter)">
        Send
      </TooltipButton>,
    )
    expect(screen.getByRole('button', { name: 'Send' }).className).toContain('ui-button-primary')
  })
})
