// The menu opens from the keyboard, the arrows move, Enter chooses and
// Escape closes and gives the focus back.

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { Button } from './button'
import { Menu } from './menu'

describe('Menu', () => {
  const items = (open: () => void, remove: () => void) => [
    { label: 'Open', onSelect: open },
    { label: 'Delete', onSelect: remove, danger: true },
  ]

  it('opens on Enter, moves on ArrowDown and chooses on Enter', async () => {
    const user = userEvent.setup()
    const open = vi.fn()
    const remove = vi.fn()
    render(<Menu trigger={<Button>Actions</Button>} items={items(open, remove)} />)

    await user.tab()
    await user.keyboard('{Enter}')
    expect(await screen.findByRole('menuitem', { name: 'Open' })).toBeTruthy()

    await user.keyboard('{ArrowDown}{Enter}')
    expect(remove).toHaveBeenCalledTimes(1)
    expect(open).not.toHaveBeenCalled()
  })

  it('closes on Escape and returns the focus to the trigger', async () => {
    const user = userEvent.setup()
    render(<Menu trigger={<Button>Actions</Button>} items={items(vi.fn(), vi.fn())} />)

    await user.tab()
    await user.keyboard('{Enter}')
    expect(await screen.findByRole('menuitem', { name: 'Open' })).toBeTruthy()

    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menuitem', { name: 'Open' })).toBeNull()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Actions' }))
  })
})
