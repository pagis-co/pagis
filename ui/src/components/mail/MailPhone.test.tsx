import { QueryClient } from '@tanstack/react-query'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { renderInRouter } from '../../test/router'
import { Mail } from './Mail'

const native = vi.hoisted(() => ({ on: false, write: vi.fn(), open: vi.fn() }))
vi.mock('@capacitor/core', () => ({ Capacitor: { isNativePlatform: () => native.on } }))
vi.mock('@capacitor/filesystem', () => ({ Directory: { Cache: 'CACHE' }, Filesystem: { writeFile: native.write } }))
vi.mock('@capacitor/file-viewer', () => ({ FileViewer: { openDocumentFromLocalPath: native.open } }))
vi.mock('../../primitives/useMediaQuery', () => ({ useIsMobile: () => true }))

const message = {
  id: 'INBOX:1', thread_id: 'thread-1', from: 'sender@example.test',
  subject: 'Invoice', text: 'Please read.', snippet: 'Please read.', date: 0,
  seen: false, labels: [], spam_verdict: { score: 0, action: 'accept', reasons: [] },
  attachments: [{ name: 'invoice.txt', bytes: 3 }],
}
function mount() {
  const GET = vi.fn(async (path: string, options?: { params?: { query?: { action?: string } } }) => {
    if (path.endsWith('/mailbox')) return { data: { mailbox: { address: 'mail@example.test' } } }
    if (path.endsWith('/folders')) return { data: { items: ['INBOX', 'Paid'] } }
    if (path.endsWith('/composer')) {
      const action = options?.params?.query?.action
      return { data: { to: action === 'forward' ? [] : ['sender@example.test'], cc: action === 'reply-all' ? ['copy@example.test'] : [], subject: action === 'forward' ? 'Fwd: Invoice' : 'Re: Invoice', body: '> Please read.' } }
    }
    if (path.endsWith('/{message_id}')) return { data: message }
    return { data: { items: [message] } }
  })
  const POST = vi.fn(async () => ({ data: { message_id: 'sent' } }))
  const history = renderInRouter(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><Mail api={{ GET, POST } as unknown as ApiClient} /></QueryClientProvider>)
  return { GET, POST, history }
}
async function allMail() {
  fireEvent.click(await screen.findByRole('button', { name: 'All mail' }))
  return screen.findByRole('button', { name: /Invoice/ })
}
async function openMessage() { fireEvent.click(await allMail()); await screen.findByRole('heading', { name: 'Invoice' }) }
beforeEach(() => {
  native.on = false
  native.write.mockReset().mockResolvedValue({ uri: 'file:///cache/invoice.txt' })
  native.open.mockReset().mockResolvedValue(undefined)
})
afterEach(() => vi.unstubAllGlobals())

it('shows the phone back to You, views and Compose action', async () => {
  const { history } = mount()
  expect(await screen.findByText('No mail needs you.')).toBeTruthy()
  const compose = screen.getByRole('button', { name: 'Compose' })
  expect(compose.closest('.nav-bar')).toBeTruthy()
  fireEvent.click(compose)
  const sheet = screen.getByRole('dialog', { name: 'Compose' })
  expect(within(sheet).getByRole('button', { name: 'Send' })).toBeTruthy()
  fireEvent.click(within(sheet).getByRole('button', { name: 'Cancel' }))
  fireEvent.click(screen.getByRole('button', { name: 'You' }))
  await waitFor(() => expect(history.location.pathname).toBe('/you'))
})
it('switches views and changes the search and folder query', async () => {
  const { GET } = mount()
  await allMail()
  expect(screen.getByRole('button', { name: 'All mail' }).getAttribute('aria-pressed')).toBe('true')
  fireEvent.change(screen.getByLabelText('Search mail'), { target: { value: 'invoice' } })
  fireEvent.keyDown(screen.getByRole('combobox', { name: 'Folder' }), { key: 'ArrowDown' })
  fireEvent.click(await screen.findByRole('option', { name: 'Paid' }))
  await waitFor(() => expect(GET).toHaveBeenCalledWith('/api/v1/workspace/mail/messages', { params: { query: { folder: 'Paid', q: 'invoice', cursor: undefined } } }))
  fireEvent.click(screen.getByRole('button', { name: 'Needs you' }))
  expect(screen.getByText('No mail needs you.')).toBeTruthy()
})
it('opens a thread and returns to the list with the Mail back control', async () => {
  mount(); await openMessage()
  expect(screen.queryByLabelText('Search mail')).toBeNull()
  expect(screen.queryByRole('button', { name: 'You' })).toBeNull()
  expect(screen.queryByRole('navigation', { name: 'Mail views' })).toBeNull()
  expect(screen.getByRole('button', { name: 'Move' })).toBeTruthy()
  const back = screen.getByRole('button', { name: 'Mail' })
  expect(back.closest('.nav-bar')).toBeTruthy()
  fireEvent.click(back)
  expect(await screen.findByLabelText('Search mail')).toBeTruthy()
})
for (const [label, action] of [['Reply', 'reply'], ['Reply all', 'reply-all'], ['Forward', 'forward']]) {
  it(`opens a prefilled ${label} sheet and sends`, async () => {
    const { POST } = mount(); await openMessage()
    fireEvent.click(screen.getByRole('button', { name: label }))
    const sheet = await screen.findByRole('dialog', { name: label })
    const to = await within(sheet).findByLabelText('To') as HTMLInputElement
    expect(to.value).toBe(action === 'forward' ? '' : 'sender@example.test')
    expect((within(sheet).getByLabelText('Cc') as HTMLInputElement).value).toBe(action === 'reply-all' ? 'copy@example.test' : '')
    expect((within(sheet).getByLabelText('Body') as HTMLTextAreaElement).value).toBe('> Please read.')
    fireEvent.change(to, { target: { value: 'reader@example.test' } })
    fireEvent.click(within(sheet).getByRole('button', { name: 'Send' }))
    await waitFor(() => expect(POST).toHaveBeenCalledWith(`/api/v1/workspace/mail/messages/{message_id}/${action}`, expect.anything()))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })
}
it('opens folder management in a sheet', async () => {
  mount(); await allMail()
  expect(screen.queryByLabelText('Folder name')).toBeNull()
  fireEvent.click(screen.getByRole('button', { name: 'Folders' }))
  expect(within(screen.getByRole('dialog', { name: 'Folders' })).getByLabelText('Folder name')).toBeTruthy()
})
it('opens browser attachments in a new tab without download', async () => {
  mount(); await openMessage()
  const link = screen.getByRole('link', { name: 'invoice.txt · 3 B' })
  expect(link.getAttribute('target')).toBe('_blank')
  expect(link.getAttribute('rel')).toBe('noopener')
  expect(link.hasAttribute('download')).toBe(false)
})
it('fetches native attachments with the session cookie and opens the cached bytes', async () => {
  native.on = true
  const fetch = vi.fn(async () => new Response(new Uint8Array([0, 128, 255])))
  vi.stubGlobal('fetch', fetch)
  mount(); await openMessage()
  fireEvent.click(screen.getByRole('button', { name: /invoice.txt/ }))
  await waitFor(() => expect(native.open).toHaveBeenCalledWith({ path: '/cache/invoice.txt' }))
  expect(fetch).toHaveBeenCalledWith('/api/v1/workspace/mail/messages/INBOX%3A1/attachments/0', { credentials: 'include' })
  expect(native.write).toHaveBeenCalledWith(expect.objectContaining({ path: expect.stringMatching(/invoice.txt$/), data: 'AID/', directory: 'CACHE' }))
})
it.each(['fetch', 'write', 'viewer'])('shows an alert when attachment %s fails', async (step) => {
  native.on = true
  vi.stubGlobal('fetch', vi.fn(async () => new Response('abc', { status: step === 'fetch' ? 500 : 200 })))
  if (step === 'write') native.write.mockRejectedValue(new Error('Disk full'))
  if (step === 'viewer') native.open.mockRejectedValue(new Error('No viewer'))
  mount(); await openMessage()
  fireEvent.click(screen.getByRole('button', { name: /invoice.txt/ }))
  expect((await screen.findByRole('alert')).textContent).toContain('Could not open the attachment')
})
