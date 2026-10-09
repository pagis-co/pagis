// On the phone a message shows its time in the clock of the reader's
// locale, the same as on the desktop.

import { render, screen } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { formatClock } from '../timeline'
import { MessageRow } from './MessageRow'

afterEach(() => vi.unstubAllGlobals())

it('writes the time in the locale clock on the phone', () => {
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: query.includes('max-width'),
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  }))
  const toLocaleTimeString = vi.spyOn(Date.prototype, 'toLocaleTimeString')
  const createdAt = Date.parse('2026-09-05T14:58:00Z')

  render(
    <MessageRow
      api={{} as ApiClient}
      row={{
        key: 'msg-1',
        kind: 'message',
        authorKind: 'user',
        authorAgentId: null,
        createdAt,
        sendState: 'sent',
        status: 'complete',
        runId: null,
        blocks: [{ type: 'markdown', text: 'hi' }],
        text: 'hi',
        completedAt: null,
        replyCount: 0,
        lastReplyAt: null,
        replyAuthors: [],
      }}
      onRetry={() => {}}
    />,
  )

  expect(screen.getByText(formatClock(createdAt))).toBeTruthy()
  expect(toLocaleTimeString.mock.calls.every(([locale]) => locale === undefined)).toBe(true)
  toLocaleTimeString.mockRestore()
})
