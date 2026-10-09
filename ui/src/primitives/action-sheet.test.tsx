// The action sheet confirms a destructive act. Cancel takes the focus
// first, so Enter on an opened sheet does no harm.

import { useState } from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { ActionSheet } from './action-sheet'
import { Button } from './button'

function Harness({ onSelect }: { onSelect: () => void }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Button onClick={() => setOpen(true)}>Hang up</Button>
      <ActionSheet
        open={open}
        onOpenChange={setOpen}
        title="Hang up this call?"
        description="The call ends for everyone."
        action={{ label: 'Hang up', danger: true, onSelect }}
      />
    </>
  )
}

describe('ActionSheet', () => {
  it('opens with the focus on Cancel', async () => {
    const user = userEvent.setup()
    render(<Harness onSelect={() => {}} />)

    await user.click(screen.getByRole('button', { name: 'Hang up' }))

    const sheet = await screen.findByRole('alertdialog', { name: 'Hang up this call?' })
    expect(sheet.textContent).toContain('The call ends for everyone.')
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Cancel' })))
  })

  it('fires the action and closes', async () => {
    const user = userEvent.setup()
    const onSelect = vi.fn()
    render(<Harness onSelect={onSelect} />)
    await user.click(screen.getByRole('button', { name: 'Hang up' }))
    const sheet = await screen.findByRole('alertdialog')

    await user.click(screen.getAllByRole('button', { name: 'Hang up' }).find((button) => sheet.contains(button)) as HTMLElement)

    expect(onSelect).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
  })

  it('closes on Cancel and does not fire the action', async () => {
    const user = userEvent.setup()
    const onSelect = vi.fn()
    render(<Harness onSelect={onSelect} />)
    await user.click(screen.getByRole('button', { name: 'Hang up' }))
    await screen.findByRole('alertdialog')

    await user.click(screen.getByRole('button', { name: 'Cancel' }))

    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(onSelect).not.toHaveBeenCalled()
  })
})
