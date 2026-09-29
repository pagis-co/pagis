import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, test, vi } from 'vitest'
import { ForgetControl } from './ForgetControl'

test('forget requires reviewing the closure and keeps relearning separate from deletion', async () => {
  const preview = vi.fn().mockResolvedValue({ revision: 'sql-1', memory_revision: 'git-1', source_items: 2, memory_paths: 2, memory_revisions: 4, raw_account_retrieval_blocked: true })
  const confirm = vi.fn().mockResolvedValue({ id: 'f1', connection_id: 'c1', phase: 'blocked', error: null, created_at: 1, reopted_at: null })
  const retry = vi.fn()
  const reopt = vi.fn().mockResolvedValue(undefined)
  const props = { label: 'this account', preview, confirm, retry, reopt }
  const view = render(<ForgetControl {...props} />)
  expect(screen.queryByRole('button', { name: 'Confirm forget' })).toBeNull()
  await userEvent.click(screen.getByRole('button', { name: 'Preview what goes' }))
  await screen.findByText(/2 imported source items/)
  expect(screen.getByText(/raw account retrieval/i)).toBeTruthy()
  expect(screen.getByText(/providers.*logs.*backups.*delivered messages/i)).toBeTruthy()
  expect(
    screen.getByText(/Disconnecting or pausing acquisition does not perform this deletion/i),
  ).toBeTruthy()
  expect(confirm).not.toHaveBeenCalled()
  await userEvent.click(screen.getByRole('button', { name: 'Confirm forget' }))
  await screen.findByText(/blocked now/i)
  expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ revision: 'sql-1', memory_revision: 'git-1' }))
  expect(screen.queryByRole('button', { name: 'Allow learning again' })).toBeNull()
  view.rerender(<ForgetControl {...props} operation={{ id: 'f1', connection_id: 'c1', phase: 'complete', error: null, created_at: 1, reopted_at: null }} />)
  expect(screen.getByText(/suppression remains active/i)).toBeTruthy()
  expect(reopt).not.toHaveBeenCalled()
  await userEvent.click(screen.getByRole('button', { name: 'Allow learning again' }))
  await waitFor(() => expect(reopt).toHaveBeenCalledWith('f1'))
})

test('failed deletion stays blocked and offers an explicit retry', async () => {
  const retry = vi.fn().mockResolvedValue(undefined)
  render(<ForgetControl label="this account" preview={vi.fn()} confirm={vi.fn()} retry={retry} reopt={vi.fn()}
    operation={{ id: 'f2', connection_id: 'c1', phase: 'structured_purged', error: 'Memory storage unavailable', created_at: 1, reopted_at: null }} />)
  expect(screen.getByRole('alert').textContent).toMatch(/still blocked/i)
  expect(screen.queryByRole('button', { name: 'Allow learning again' })).toBeNull()
  await userEvent.click(screen.getByRole('button', { name: 'Retry deletion' }))
  await waitFor(() => expect(retry).toHaveBeenCalledWith('f2'))
})
