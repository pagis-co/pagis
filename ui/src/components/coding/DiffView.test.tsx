// One change of a Coding Session as a unified diff.

import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { DiffView } from './DiffView'
import type { DiffContent } from './transcript'

function diff(fields: Partial<DiffContent> = {}): DiffContent {
  return {
    type: 'diff',
    path: '/repo/src/login.rs',
    oldText: 'one\ntwo\nthree\n',
    newText: 'one\n2\nthree\n',
    truncated: false,
    ...fields,
  }
}

function texts(container: HTMLElement, tag: string): string[] {
  return [...container.querySelectorAll(tag)].map((element) => element.textContent ?? '')
}

describe('a change', () => {
  it('shows its path, the removed lines as del and the added lines as ins', () => {
    const { container } = render(<DiffView content={diff()} />)

    expect(screen.getByText('/repo/src/login.rs')).toBeTruthy()
    expect(texts(container, 'del')).toEqual(['two'])
    expect(texts(container, 'ins')).toEqual(['2'])
    expect(screen.getByText('one')).toBeTruthy()
    expect(screen.queryByText('New file')).toBeNull()
  })

  it('says a file is new', () => {
    render(<DiffView content={diff({ oldText: null, newText: 'fn main() {}\n' })} />)

    expect(screen.getByText('New file')).toBeTruthy()
  })

  it('says a file is deleted', () => {
    render(<DiffView content={diff({ newText: '' })} />)

    expect(screen.getByText('Deleted')).toBeTruthy()
  })

  it('folds a change of more than 80 lines and opens it with Show all', () => {
    const newText = Array.from({ length: 100 }, (_, n) => `line ${n}`).join('\n') + '\n'
    const { container } = render(<DiffView content={diff({ oldText: null, newText })} />)

    expect(texts(container, 'ins')).toHaveLength(80)

    fireEvent.click(screen.getByRole('button', { name: 'Show all 100 lines' }))

    expect(texts(container, 'ins')).toHaveLength(100)
    expect(screen.queryByRole('button', { name: /Show all/ })).toBeNull()
  })

  it('has no Show all for a change of 80 lines or less', () => {
    const newText = Array.from({ length: 80 }, (_, n) => `line ${n}`).join('\n') + '\n'
    render(<DiffView content={diff({ oldText: null, newText })} />)

    expect(screen.queryByRole('button', { name: /Show all/ })).toBeNull()
  })

  it('says that a cut change is too large to show', () => {
    const { container } = render(<DiffView content={diff({ newText: 'one…', truncated: true })} />)

    expect(screen.getByText('/repo/src/login.rs')).toBeTruthy()
    expect(screen.getByText('This change is too large to show here.')).toBeTruthy()
    expect(container.querySelectorAll('del, ins')).toHaveLength(0)
  })
})
