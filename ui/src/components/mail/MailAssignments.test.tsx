import { QueryClient } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { MailAssignments, AssignmentFeed, AssignmentSettings } from './MailAssignments'

const assignment = { id: 'a1', assignee: 'person1', role: 'fyi', state: 'assigned', summary: 'The proposal is ready.', channel_id: 'c1' }
function show(node: React.ReactNode) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>)
}
it('shows the assignments and states of a message', async () => {
  const GET = vi.fn(async () => ({ data: { items: [assignment] } }))
  show(<MailAssignments api={{ GET } as unknown as ApiClient} messageId="INBOX:1" />)
  expect(await screen.findByText(/The proposal is ready/)).toBeTruthy()
  expect(screen.getByText(/fyi · assigned/)).toBeTruthy()
  expect(screen.getByRole('link', { name: 'Open assignment' }).getAttribute('href')).toBe('/conversations/c1')
})
it('shows FYI entries and dismisses one', async () => {
  const GET = vi.fn(async () => ({ data: { items: [assignment] } }))
  const POST = vi.fn(async () => ({ data: {} }))
  show(<AssignmentFeed api={{ GET, POST } as unknown as ApiClient} />)
  expect(await screen.findByText('The proposal is ready.')).toBeTruthy()
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss FYI' }))
  await waitFor(() => expect(POST).toHaveBeenCalledWith('/api/v1/mail-assignments/{assignment_id}/done', { params: { path: { assignment_id: 'a1' } } }))
})
it('sets and clears the assignment limit and sets the timeout', async () => {
  const GET = vi.fn(async () => ({ data: { assignment_limit: null, assignment_timeout_minutes: 1440 } }))
  const PUT = vi.fn(async () => ({ data: {} }))
  show(<AssignmentSettings api={{ GET, PUT } as unknown as ApiClient} />)
  const limit = await screen.findByLabelText('Assignment limit per member')
  fireEvent.change(limit, { target: { value: '2' } })
  fireEvent.change(screen.getByLabelText('Assignment timeout in minutes'), { target: { value: '30' } })
  fireEvent.click(screen.getByRole('button', { name: 'Save assignment settings' }))
  await waitFor(() => expect(PUT).toHaveBeenLastCalledWith('/api/v1/workspace/mailbox/assignment-settings', { body: { assignment_limit: 2, assignment_timeout_minutes: 30 } }))
  fireEvent.change(limit, { target: { value: '' } })
  fireEvent.click(screen.getByRole('button', { name: 'Save assignment settings' }))
  await waitFor(() => expect(PUT).toHaveBeenLastCalledWith('/api/v1/workspace/mailbox/assignment-settings', { body: { assignment_limit: null, assignment_timeout_minutes: 30 } }))
})

it('lets the assignee mark an escalated assignment done in its conversation', async () => {
  const { ConversationAssignment } = await import('./MailAssignments')
  const GET = vi.fn(async () => ({ data: { ...assignment, state: 'escalated' } }))
  const POST = vi.fn(async () => ({ data: {} }))
  show(<ConversationAssignment api={{ GET, POST } as unknown as ApiClient} conversationId="c1" personId="person1" />)
  fireEvent.click(await screen.findByRole('button', { name: 'Mark assignment done' }))
  await waitFor(() => expect(POST).toHaveBeenCalledWith('/api/v1/mail-assignments/{assignment_id}/done', { params: { path: { assignment_id: 'a1' } } }))
})

it('keeps cached private FYI entries out of a Shared Workspace Home', async () => {
  const { WorkspaceSelectionContext } = await import('../../workspace')
  const client = new QueryClient()
  client.setQueryData(['assignment-feed'], { items: [assignment] })
  const GET = vi.fn()
  const current = { id: 'shared', name: 'Support', kind: 'shared' as const, role: 'manager' as const }
  render(<QueryClientProvider client={client}><WorkspaceSelectionContext value={{ items: [current], current, select: () => {} }}><AssignmentFeed api={{ GET } as unknown as ApiClient} /></WorkspaceSelectionContext></QueryClientProvider>)
  expect(screen.queryByText('The proposal is ready.')).toBeNull()
  expect(GET).not.toHaveBeenCalled()
})
