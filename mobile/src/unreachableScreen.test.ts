import { readFileSync } from 'node:fs'
import path from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { mountUnreachableScreen, type UnreachableScreenActions } from './unreachableScreen'

const PAGE = readFileSync(path.join(import.meta.dirname, '..', 'unreachable.html'), 'utf8')

const QUERY = '?server=https%3A%2F%2Fa.example&error=A+server+with+the+specified+hostname+could+not+be+found.'

function button(id: string): HTMLButtonElement {
  return document.querySelector<HTMLButtonElement>(`#${id}`)!
}

function problem(): HTMLElement {
  return document.querySelector<HTMLElement>('#problem')!
}

function actions(overrides: Partial<UnreachableScreenActions> = {}): UnreachableScreenActions {
  return { open: vi.fn(async () => {}), changeServer: vi.fn(async () => {}), ...overrides }
}

async function press(id: string): Promise<void> {
  button(id).click()
  await vi.waitFor(() => expect(button(id).disabled).toBe(false))
}

describe('the Unreachable screen', () => {
  beforeEach(() => {
    document.documentElement.innerHTML = new DOMParser().parseFromString(PAGE, 'text/html').documentElement.innerHTML
  })

  it('names the server and the error, and shows Try again and Change server', () => {
    mountUnreachableScreen(document, QUERY, actions())

    expect(document.querySelector('#server')?.textContent).toBe('https://a.example')
    expect(document.querySelector('#error')?.textContent).toBe('A server with the specified hostname could not be found.')
    expect(button('retry').textContent?.trim()).toBe('Try again')
    expect(button('change').textContent?.trim()).toBe('Change server')
    expect(problem().hidden).toBe(true)
  })

  it('shows the text of the page as text, not as markup', () => {
    mountUnreachableScreen(document, '?server=https%3A%2F%2Fa.example&error=%3Cb%3Ex%3C%2Fb%3E', actions())

    expect(document.querySelector('#error')?.textContent).toBe('<b>x</b>')
    expect(document.querySelector('#error b')).toBeNull()
  })

  it('opens the server again on Try again', async () => {
    const shell = actions()
    mountUnreachableScreen(document, QUERY, shell)

    await press('retry')

    expect(shell.open).toHaveBeenCalledWith({ origin: 'https://a.example', opens: 'https://a.example' })
    expect(shell.changeServer).not.toHaveBeenCalled()
  })

  it('changes the server on Change server', async () => {
    const shell = actions()
    mountUnreachableScreen(document, QUERY, shell)

    await press('change')

    expect(shell.changeServer).toHaveBeenCalledOnce()
    expect(shell.open).not.toHaveBeenCalled()
  })

  it('shows a refusal of the shell as a problem', async () => {
    const shell = actions({ open: vi.fn(async () => Promise.reject(new Error('This is not the origin of a Pagis server that the app opens.'))) })
    mountUnreachableScreen(document, QUERY, shell)

    await press('retry')

    expect(problem().hidden).toBe(false)
    expect(problem().textContent).toBe('This is not the origin of a Pagis server that the app opens.')
  })
})
