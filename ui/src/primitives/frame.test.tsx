// The Frame is the bordered surface of a settings list and the Row is
// one line in it. The last row draws no divider, and a hint
// reads under the frame, outside its border.

import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { Frame, Row } from './frame'

describe('Frame', () => {
  it('draws one surface with the rows inside it', () => {
    const { container } = render(
      <Frame>
        <Row>First</Row>
        <Row>Last</Row>
      </Frame>,
    )
    const frame = container.firstElementChild as HTMLElement
    expect(frame.className).toContain('ui-frame')
    expect(frame.querySelectorAll('.ui-row')).toHaveLength(2)
  })

  it('writes the hint under the frame, outside its border', () => {
    const { container } = render(
      <Frame hint="The password stays in the keychain.">
        <Row>lufthansa.com</Row>
      </Frame>,
    )
    const hint = screen.getByText('The password stays in the keychain.')
    expect(hint.className).toContain('ui-frame-hint')
    expect(hint.closest('.ui-frame')).toBeNull()
    expect(container.querySelector('.ui-frame')).not.toBeNull()
  })

  it('renders no hint element without a hint', () => {
    const { container } = render(
      <Frame>
        <Row>One</Row>
      </Frame>,
    )
    expect(container.querySelector('.ui-frame-hint')).toBeNull()
  })
})

describe('Row', () => {
  it('carries the screen class next to its own', () => {
    render(
      <Frame>
        <Row className="vault-row">acme.slack.com</Row>
      </Frame>,
    )
    const row = screen.getByText('acme.slack.com')
    expect(row.className).toContain('ui-row')
    expect(row.className).toContain('vault-row')
  })
})
