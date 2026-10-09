// The Frame is the bordered surface of a settings list and the Row is
// one line in it. The last row draws no divider, and a hint
// reads under the frame, outside its border.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { renderInRouter } from '../test/router'
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

describe('Row with a chevron', () => {
  it('renders one button with an accessible name when it has an action', () => {
    const onClick = vi.fn()
    render(
      <Frame>
        <Row chevron onClick={onClick}>
          Trusted contacts
        </Row>
      </Frame>,
    )

    const row = screen.getByRole('button', { name: 'Trusted contacts' })
    expect(screen.getAllByRole('button')).toHaveLength(1)
    fireEvent.click(row)
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it('opens its place in the app with no new page load', async () => {
    const history = renderInRouter(
      <Frame>
        <Row chevron href="/runs/run-1">
          Book the Austin trip
        </Row>
      </Frame>,
    )

    const link = await screen.findByRole('link', { name: 'Book the Austin trip' })
    expect(link.getAttribute('href')).toBe('/runs/run-1')
    expect(fireEvent.click(link)).toBe(false)
    await waitFor(() => expect(history.location.pathname).toBe('/runs/run-1'))
    expect(await screen.findByTestId('run-view')).toBeTruthy()
  })

  // A row with no value has no element that takes the free room, so
  // the chevron takes it and sits at the trailing edge.
  it('puts the chevron at the trailing edge', () => {
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'frame.css'), 'utf8')
    expect(css).toMatch(/\.ui-row-chevron\s*\{[^}]*margin-left:\s*auto/)
    expect(css).toMatch(/\.ui-row-value\s*\+\s*\.ui-row-chevron\s*\{[^}]*margin-left:\s*0/)
  })
})

describe('a disabled Row', () => {
  it('turns off its button', () => {
    render(
      <Row disabled onClick={() => {}}>
        Delete
      </Row>,
    )
    expect((screen.getByRole('button', { name: 'Delete' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('writes no disabled attribute on a row with no action', () => {
    render(<Row disabled>Read only</Row>)
    expect(screen.getByText('Read only').hasAttribute('disabled')).toBe(false)
  })
})
