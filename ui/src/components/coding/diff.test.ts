// A change of a Coding Session as the session page draws it: the
// hunks and the counts of one diff, and one entry for each changed file.

import { describe, expect, it } from 'vitest'

import type { CodingSessionEventDto } from '../../api/client'
import { changedFiles, fileChange } from './diff'
import { foldTranscript, type DiffContent } from './transcript'

function diff(
  path: string,
  oldText: string | null,
  newText: string,
  truncated = false,
): DiffContent {
  return { type: 'diff', path, oldText, newText, truncated }
}

describe('one change', () => {
  it('gives the hunks and the counts of an edit', () => {
    const change = fileChange(diff('/repo/a.rs', 'one\ntwo\nthree\n', 'one\n2\nthree\nfour\n'))

    expect(change).toEqual({
      path: '/repo/a.rs',
      tooLarge: false,
      created: false,
      deleted: false,
      added: 2,
      removed: 1,
      hunks: [
        [
          { kind: 'same', text: 'one' },
          { kind: 'removed', text: 'two' },
          { kind: 'added', text: '2' },
          { kind: 'same', text: 'three' },
          { kind: 'added', text: 'four' },
        ],
      ],
    })
  })

  it('keeps three unchanged lines around a change and splits far changes into hunks', () => {
    const before = Array.from({ length: 20 }, (_, n) => `line ${n}`).join('\n') + '\n'
    const after = before.replace('line 1\n', 'line one\n').replace('line 18\n', 'line eighteen\n')

    const change = fileChange(diff('/repo/long.txt', before, after))
    if (change.tooLarge) throw new Error('the change is not too large')

    expect(change.hunks).toHaveLength(2)
    expect(change.hunks[0].map((line) => line.text)).toEqual([
      'line 0',
      'line 1',
      'line one',
      'line 2',
      'line 3',
      'line 4',
    ])
  })

  it('marks a file with no old text as new', () => {
    const change = fileChange(diff('/repo/new.rs', null, 'fn main() {}\n'))

    expect(change).toMatchObject({ created: true, deleted: false, added: 1, removed: 0 })
  })

  it('marks a file with an empty new text as deleted', () => {
    const change = fileChange(diff('/repo/old.rs', 'a\nb\n', ''))

    expect(change).toMatchObject({ created: false, deleted: true, added: 0, removed: 2 })
  })

  it('shows no marker of a missing last newline as a line', () => {
    const change = fileChange(diff('/repo/a.rs', 'a', 'b'))
    if (change.tooLarge) throw new Error('the change is not too large')

    expect(change.hunks).toEqual([
      [
        { kind: 'removed', text: 'a' },
        { kind: 'added', text: 'b' },
      ],
    ])
  })

  it('gives no hunks for a diff in a payload that the daemon cut', () => {
    const change = fileChange(diff('/repo/big.rs', 'a\n', 'b…', true))

    expect(change).toEqual({ path: '/repo/big.rs', tooLarge: true })
  })
})

function row(
  seq: number,
  kind: CodingSessionEventDto['kind'],
  payload: Record<string, unknown>,
): CodingSessionEventDto {
  return { seq, at: seq, kind, payload }
}

describe('the changed files', () => {
  it('folds two changes of one path into one entry with the summed counts and the last tool call', () => {
    const { items } = foldTranscript([
      row(1, 'tool_call', {
        toolCallId: 'call-1',
        kind: 'edit',
        content: [{ type: 'diff', path: '/repo/a.rs', oldText: 'a\n', newText: 'b\n' }],
      }),
      row(2, 'tool_call', {
        toolCallId: 'call-2',
        kind: 'edit',
        content: [{ type: 'diff', path: '/repo/b.rs', oldText: null, newText: 'x\ny\n' }],
      }),
      row(3, 'tool_call', {
        toolCallId: 'call-3',
        kind: 'edit',
        content: [{ type: 'diff', path: '/repo/a.rs', oldText: 'b\n', newText: 'b\nc\nd\n' }],
      }),
    ])

    expect(changedFiles(items)).toEqual([
      { path: '/repo/a.rs', changes: 2, added: 3, removed: 1, toolCallId: 'call-3' },
      { path: '/repo/b.rs', changes: 1, added: 2, removed: 0, toolCallId: 'call-2' },
    ])
  })

  it('counts a cut change but no lines of it', () => {
    const { items } = foldTranscript([
      row(1, 'tool_call', {
        toolCallId: 'call-1',
        kind: 'edit',
        truncated: true,
        content: [{ type: 'diff', path: '/repo/a.rs', oldText: 'a\n', newText: 'b…' }],
      }),
    ])

    expect(changedFiles(items)).toEqual([
      { path: '/repo/a.rs', changes: 1, added: 0, removed: 0, toolCallId: 'call-1' },
    ])
  })

  it('is empty when the session changed no file', () => {
    const { items } = foldTranscript([row(1, 'prompt', { text: 'Hello', message_id: null })])

    expect(changedFiles(items)).toEqual([])
  })
})
