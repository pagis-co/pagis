import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ApiClient } from './api/client'
import { BrowserSignInPage } from './BrowserSignInPage'

const challenge = 'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM'
afterEach(() => window.history.replaceState(null, '', '/'))

describe('the browser sign-in of the Mobile App', () => {
  it.each(['password', 'link'])('returns a bound Sign-In Link after %s sign-in', async (method) => {
    window.history.replaceState(null, '', `/sign-in?app=ios&state=random-state&challenge=${challenge}`)
    const api = {
      GET: vi.fn(async (path: string) => path.endsWith('/health')
        ? { data: { sign_in: method } }
        : { data: undefined, response: new Response(null, { status: 401 }) }),
      POST: vi.fn(async (path: string) => ({ data: path.endsWith('/sign-in-links')
        ? { url: 'https://pagis.example/sign-in#bound-secret', expires_at: 123, qr_svg: '' }
        : { id: 'person' }, response: new Response(null, { status: 200 }) })),
      DELETE: vi.fn(async () => ({ response: new Response(null, { status: 204 }) })),
    }
    const redirect = vi.fn()
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={client}><BrowserSignInPage api={api as unknown as ApiClient} redirect={redirect} /></QueryClientProvider>)
    if (method === 'password') {
      fireEvent.change(await screen.findByLabelText('Email'), { target: { value: 'person@example.com' } })
      fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'password' } })
    } else {
      fireEvent.change(await screen.findByLabelText('Paste a sign-in link'), { target: { value: 'pasted-secret' } })
    }
    fireEvent.submit(screen.getByRole('form', { name: 'Sign in' }))
    await waitFor(() => expect(redirect).toHaveBeenCalledWith('pagis-app://signed-in?state=random-state#bound-secret'))
    expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/sign-in-links', { body: { challenge } })
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/sessions/current')
    expect(api.DELETE.mock.invocationCallOrder[0]).toBeLessThan(redirect.mock.invocationCallOrder[0])
    expect(screen.getByRole('link', { name: 'Return to the app' }).getAttribute('href')).toBe('pagis-app://signed-in?state=random-state#bound-secret')
  })

  it.each(['password', 'link'])('returns a desktop bound Sign-In Link after %s sign-in', async (method) => {
    window.history.replaceState(null, '', `/sign-in?app=desktop&state=random-state&challenge=${challenge}`)
    const api = {
      GET: vi.fn(async (path: string) => path.endsWith('/health')
        ? { data: { sign_in: method } }
        : { data: undefined, response: new Response(null, { status: 401 }) }),
      POST: vi.fn(async (path: string) => ({ data: path.endsWith('/sign-in-links')
        ? { url: 'https://pagis.example/sign-in#bound-secret', expires_at: 123, qr_svg: '' }
        : { id: 'person' }, response: new Response(null, { status: 200 }) })),
      DELETE: vi.fn(async () => ({ response: new Response(null, { status: 204 }) })),
    }
    const redirect = vi.fn()
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={client}><BrowserSignInPage api={api as unknown as ApiClient} redirect={redirect} /></QueryClientProvider>)
    if (method === 'password') {
      fireEvent.change(await screen.findByLabelText('Email'), { target: { value: 'person@example.com' } })
      fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'password' } })
    } else {
      fireEvent.change(await screen.findByLabelText('Paste a sign-in link'), { target: { value: 'pasted-secret' } })
    }
    fireEvent.submit(screen.getByRole('form', { name: 'Sign in' }))
    await waitFor(() => expect(redirect).toHaveBeenCalledWith('pagis://auth?state=random-state#bound-secret'))
    expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/sign-in-links', { body: { challenge } })
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/sessions/current')
    expect(api.DELETE.mock.invocationCallOrder[0]).toBeLessThan(redirect.mock.invocationCallOrder[0])
    expect(screen.getByRole('link', { name: 'Return to the app' }).getAttribute('href')).toBe('pagis://auth?state=random-state#bound-secret')
  })

  it.each(['app=other&state=s&challenge='+challenge, 'app=ios&challenge='+challenge, 'app=ios&state=s&challenge=bad'])('refuses incomplete or invalid parameters: %s', async (query) => {
    window.history.replaceState(null, '', `/sign-in?${query}`)
    const api = { GET: vi.fn(), POST: vi.fn() }
    render(<QueryClientProvider client={new QueryClient()}><BrowserSignInPage api={api as unknown as ApiClient} /></QueryClientProvider>)
    expect(await screen.findByRole('alert')).toBeTruthy()
    expect(api.POST).not.toHaveBeenCalled()
  })
})

it('keeps Return to the app when the browser refuses the custom scheme', async () => {
  window.history.replaceState(null, '', `/sign-in?app=android&state=s&challenge=${challenge}`)
  const api = {
    GET: vi.fn(async () => ({ data: { id: 'person' } })),
    POST: vi.fn(async () => ({ data: { url: 'https://pagis.example/sign-in#secret' } })),
    DELETE: vi.fn(async () => ({ response: new Response(null, { status: 204 }) })),
  }
  render(<QueryClientProvider client={new QueryClient()}><BrowserSignInPage api={api as unknown as ApiClient} redirect={() => { throw new Error('blocked') }} /></QueryClientProvider>)
  expect(await screen.findByRole('link', { name: 'Return to the app' })).toBeTruthy()
})

it('ends the browser Session when making the bound link fails', async () => {
  window.history.replaceState(null, '', `/sign-in?app=android&state=s&challenge=${challenge}`)
  const api = {
    GET: vi.fn(async () => ({ data: { id: 'person' } })),
    POST: vi.fn(async () => ({ data: undefined })),
    DELETE: vi.fn(async () => ({ response: new Response(null, { status: 204 }) })),
  }
  const redirect = vi.fn()
  render(<QueryClientProvider client={new QueryClient()}><BrowserSignInPage api={api as unknown as ApiClient} redirect={redirect} /></QueryClientProvider>)
  expect(await screen.findByRole('alert')).toBeTruthy()
  expect(api.DELETE).toHaveBeenCalledOnce()
  expect(redirect).not.toHaveBeenCalled()
})
