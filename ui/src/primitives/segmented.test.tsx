// The segmented control picks one of a few views. It is one tab list,
// so the arrow keys move the choice.

import { useState } from 'react'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { Segmented } from './segmented'

function Harness() {
  const [value, setValue] = useState('pages')
  return (
    <Segmented
      label="Memory view"
      value={value}
      onValueChange={setValue}
      items={[
        { value: 'pages', label: 'Pages' },
        { value: 'changes', label: 'Changes' },
        { value: 'procedures', label: 'Procedures' },
      ]}
    />
  )
}

describe('Segmented', () => {
  it('names its list and marks the chosen item', () => {
    render(<Harness />)

    expect(screen.getByRole('tablist', { name: 'Memory view' })).toBeTruthy()
    expect(screen.getByRole('tab', { name: 'Pages' }).getAttribute('aria-selected')).toBe('true')
  })

  it('moves the choice with the arrow keys', async () => {
    const user = userEvent.setup()
    render(<Harness />)
    await user.tab()
    expect(document.activeElement).toBe(screen.getByRole('tab', { name: 'Pages' }))

    await user.keyboard('{ArrowRight}')
    expect(screen.getByRole('tab', { name: 'Changes' }).getAttribute('aria-selected')).toBe('true')

    await user.keyboard('{ArrowLeft}{ArrowLeft}')
    expect(screen.getByRole('tab', { name: 'Procedures' }).getAttribute('aria-selected')).toBe('true')
  })
})
