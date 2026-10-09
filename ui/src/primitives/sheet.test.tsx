// The form sheet of the phone: a title, Cancel at the leading edge and
// the one action at the trailing edge. Cancel and Escape close it.

import { useState } from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { Button } from './button'
import { Sheet } from './sheet'

function Harness({ onSelect = () => {}, disabled = false }: { onSelect?: () => void; disabled?: boolean }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Button onClick={() => setOpen(true)}>New group</Button>
      <Sheet open={open} onOpenChange={setOpen} title="New group" action={{ label: 'Create', onSelect, disabled }}>
        <p>Choose the sprites.</p>
      </Sheet>
    </>
  )
}

describe('Sheet', () => {
  it('opens with its title as the name of the dialog', async () => {
    const user = userEvent.setup()
    render(<Harness />)

    await user.click(screen.getByRole('button', { name: 'New group' }))

    const sheet = await screen.findByRole('dialog', { name: 'New group' })
    expect(sheet.textContent).toContain('Choose the sprites.')
  })

  it('closes on Cancel and gives the focus back', async () => {
    const user = userEvent.setup()
    render(<Harness />)
    const opener = screen.getByRole('button', { name: 'New group' })
    await user.click(opener)
    await screen.findByRole('dialog')

    await user.click(screen.getByRole('button', { name: 'Cancel' }))

    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(document.activeElement).toBe(opener)
  })

  it('closes on Escape', async () => {
    const user = userEvent.setup()
    render(<Harness />)
    await user.click(screen.getByRole('button', { name: 'New group' }))
    await screen.findByRole('dialog')

    await user.keyboard('{Escape}')

    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })

  it('fires its action', async () => {
    const user = userEvent.setup()
    const onSelect = vi.fn()
    render(<Harness onSelect={onSelect} />)
    await user.click(screen.getByRole('button', { name: 'New group' }))

    await user.click(await screen.findByRole('button', { name: 'Create' }))

    expect(onSelect).toHaveBeenCalledTimes(1)
  })

  it('turns off an action that is not ready', async () => {
    const user = userEvent.setup()
    render(<Harness disabled />)
    await user.click(screen.getByRole('button', { name: 'New group' }))

    const create = (await screen.findByRole('button', { name: 'Create' })) as HTMLButtonElement
    expect(create.disabled).toBe(true)
  })
})
