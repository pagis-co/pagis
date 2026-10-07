import { readFileSync } from 'node:fs'
import path from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ServerAddress } from './address'
import { mountConnectScreen } from './connectScreen'

const PAGE = readFileSync(path.join(import.meta.dirname, '..', 'index.html'), 'utf8')

function field(): HTMLInputElement {
  return document.querySelector<HTMLInputElement>('#address')!
}

function problem(): HTMLElement {
  return document.querySelector<HTMLElement>('#problem')!
}

function connectButton(): HTMLButtonElement {
  return document.querySelector<HTMLButtonElement>('button[type=submit]')!
}

async function submit(typed: string): Promise<void> {
  field().value = typed
  document.querySelector<HTMLFormElement>('#connect')!.requestSubmit()
  await vi.waitFor(() => expect(connectButton().disabled).toBe(false))
}

const ADDRESS: ServerAddress = { origin: 'https://a.example/', opens: 'https://a.example/sign-in#abc' }

describe('the Connect screen', () => {
  beforeEach(() => {
    document.documentElement.innerHTML = new DOMParser().parseFromString(PAGE, 'text/html').documentElement.innerHTML
  })

  it('shows one field, "Server address or sign-in link", and Connect', () => {
    expect(document.querySelector('label[for=address]')?.textContent).toBe('Server address or sign-in link')
    expect(document.querySelectorAll('input')).toHaveLength(1)
    expect(connectButton().textContent).toBe('Connect')
    expect(problem().hidden).toBe(true)
  })

  it('opens the server through the shell when the checks pass', async () => {
    const connect = vi.fn(async () => ADDRESS)
    const open = vi.fn(async () => {})
    mountConnectScreen(document, { connect, open })

    await submit('https://a.example/sign-in#abc')

    expect(connect).toHaveBeenCalledWith('https://a.example/sign-in#abc')
    expect(open).toHaveBeenCalledWith(ADDRESS)
    expect(problem().hidden).toBe(true)
  })

  it('shows a problem under the field, gives the field the focus, and opens nothing', async () => {
    const open = vi.fn(async () => {})
    mountConnectScreen(document, {
      connect: async () => {
        throw new Error('No Pagis server answered at https://a.example/.')
      },
      open,
    })

    await submit('a.example')

    expect(problem().hidden).toBe(false)
    expect(problem().textContent).toBe('No Pagis server answered at https://a.example/.')
    expect(field().getAttribute('aria-invalid')).toBe('true')
    expect(document.activeElement).toBe(field())
    expect(open).not.toHaveBeenCalled()
  })

  it('clears the problem at the next try', async () => {
    let fail = true
    mountConnectScreen(document, {
      connect: async () => {
        if (fail) throw new Error('A problem.')
        return ADDRESS
      },
      open: async () => {},
    })

    await submit('a.example')
    fail = false
    await submit('a.example')

    expect(problem().hidden).toBe(true)
    expect(field().hasAttribute('aria-invalid')).toBe(false)
  })
})
