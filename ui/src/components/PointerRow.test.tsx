// The DM pointer row: it names the channel the agent posted in
// and links through to it.

import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { TimelineRow } from '../timeline'
import { PointerRow } from './PointerRow'

function pointerRow(): TimelineRow {
  return {
    key: 'msg-2',
    kind: 'pointer',
    pointer: { channelId: 'ch-2', channelTitle: 'Sage ↔ Scout' },
    authorKind: 'agent',
    authorAgentId: 'ag-1',
    createdAt: 2000,
    sendState: 'sent',
    status: 'complete',
    runId: null,
    blocks: [],
    text: 'Check the logs',
    completedAt: null,
    replyCount: 0,
    lastReplyAt: null,
    replyAuthors: [],
  }
}

describe('PointerRow', () => {
  it('links through to the source channel', () => {
    const onOpenChannel = vi.fn()
    render(<PointerRow row={pointerRow()} onOpenChannel={onOpenChannel} />)

    expect(screen.queryByText('Check the logs')).not.toBeNull()
    fireEvent.click(screen.getByText('Posted in Sage ↔ Scout'))

    expect(onOpenChannel).toHaveBeenCalledWith('ch-2')
  })
})
