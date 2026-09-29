// The dialog traps the focus, closes on Escape and gives the focus back
// to the control that opened it.

import { useState } from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { Button } from './button'
import { Dialog } from './dialog'

function Harness() {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Dialog
        trigger={<Button>Rename</Button>}
        open={open}
        onOpenChange={setOpen}
        title="Rename the agent"
        description="The name shows in every conversation."
        footer={<Button variant="primary">Save</Button>}
      >
        <Button>Body control</Button>
      </Dialog>
    </>
  )
}

describe('Dialog', () => {
  it('opens from the keyboard and holds the focus inside', async () => {
    const user = userEvent.setup()
    render(<Harness />)

    await user.tab()
    await user.keyboard('{Enter}')

    const dialog = await screen.findByRole('dialog')
    expect(dialog.textContent).toContain('Rename the agent')

    await user.tab()
    await user.tab()
    await user.tab()
    await user.tab()
    expect(dialog.contains(document.activeElement)).toBe(true)
  })

  it('closes on Escape and returns the focus to the opener', async () => {
    const user = userEvent.setup()
    render(<Harness />)

    await user.tab()
    await user.keyboard('{Enter}')
    expect(await screen.findByRole('dialog')).toBeTruthy()

    await user.keyboard('{Escape}')
    expect(screen.queryByRole('dialog')).toBeNull()
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Rename' })),
    )
  })

  it('closes with the close control', async () => {
    const user = userEvent.setup()
    render(<Harness />)

    await user.tab()
    await user.keyboard('{Enter}')
    expect(await screen.findByRole('dialog')).toBeTruthy()

    await user.click(screen.getByRole('button', { name: 'Close' }))
    expect(screen.queryByRole('dialog')).toBeNull()
  })
})
