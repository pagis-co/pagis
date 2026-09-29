// An icon control has no words, so the label is its whole accessible
// name. The tooltip shows the same words on focus.

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { X } from 'lucide-react'
import { describe, expect, it, vi } from 'vitest'

import { IconButton } from './icon-button'

describe('IconButton', () => {
  it('is named by its label and presses on Enter', async () => {
    const user = userEvent.setup()
    const press = vi.fn()
    render(<IconButton icon={X} label="Close settings" onClick={press} />)

    const button = screen.getByRole('button', { name: 'Close settings' })
    await user.tab()
    expect(document.activeElement).toBe(button)

    await user.keyboard('{Enter}')
    expect(press).toHaveBeenCalledTimes(1)
  })

  it('shows the tooltip when the focus arrives', async () => {
    const user = userEvent.setup()
    render(<IconButton icon={X} label="Close settings" />)

    await user.tab()
    expect(await screen.findByRole('tooltip')).toBeTruthy()
  })

  it('draws an icon and no text', () => {
    const { container } = render(<IconButton icon={X} label="Close settings" />)
    expect(container.querySelector('svg')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Close settings' }).textContent).toBe('')
  })
})
