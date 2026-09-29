// The tab strip is one stop on Tab, and the arrows move the selection.

import { useState } from 'react'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { Tabs } from './tabs'

const ITEMS = [
  { value: 'agents', label: 'Agents' },
  { value: 'access', label: 'Access' },
  { value: 'system', label: 'System' },
]

function Harness() {
  const [value, setValue] = useState('agents')
  return (
    <Tabs label="Settings" value={value} onValueChange={setValue} items={ITEMS}>
      <p>{`panel ${value}`}</p>
    </Tabs>
  )
}

describe('Tabs', () => {
  it('lands on the selected tab and moves it with the arrows', async () => {
    const user = userEvent.setup()
    render(<Harness />)

    await user.tab()
    expect(document.activeElement).toBe(screen.getByRole('tab', { name: 'Agents' }))

    await user.keyboard('{ArrowRight}')
    expect(screen.getByRole('tab', { name: 'Access' }).getAttribute('aria-selected')).toBe(
      'true',
    )
    expect(screen.getByText('panel access')).toBeTruthy()

    await user.keyboard('{End}')
    expect(screen.getByRole('tab', { name: 'System' }).getAttribute('aria-selected')).toBe(
      'true',
    )
  })

  it('draws the panel of the selected tab only', () => {
    render(<Harness />)
    expect(screen.getAllByRole('tabpanel')).toHaveLength(1)
    expect(screen.getByText('panel agents')).toBeTruthy()
  })
})
