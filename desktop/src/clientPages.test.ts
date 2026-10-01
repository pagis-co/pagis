// The Client App's own pages, setup (static/setup.html) and status
// (static/status.html), in a DOM with a fake of their bridge. Each page
// shows its state at the top, with its one next action. The setup page
// asks a setup question only when the state asks none.

import * as fs from 'node:fs'
import * as path from 'node:path'

import { JSDOM } from 'jsdom'
import { afterEach, describe, expect, it } from 'vitest'

import type { DaemonState } from './daemon'
import { serverOrigin } from './origin'
import type { SetupState } from './setupState'

interface Page<State = unknown> {
  calls: string[]
  document: Document
  show(state: State): void
  /** The text of every element that a person can see, in page order. */
  visibleText(): string
  /** The visible button with this label. */
  button(label: string): HTMLButtonElement | undefined
}

const pages: JSDOM[] = []
afterEach(() => { while (pages.length > 0) pages.pop()!.window.close() })

/** Open a page with a fake bridge whose calls go into `calls`. */
function openPage<State>(file: string, bridge: string, methods: string[]): Page<State> {
  const calls: string[] = []
  let listener: (state: State) => void = () => {}
  const api: Record<string, unknown> = {
    onState: (next: (state: State) => void) => { listener = next },
  }
  for (const method of methods) {
    api[method] = async (...args: unknown[]) => {
      calls.push([method, ...args.map((arg) => typeof arg === 'object' ? JSON.stringify(arg) : String(arg))].join(':'))
    }
  }
  const dom = new JSDOM(fs.readFileSync(path.join(__dirname, '..', 'static', file), 'utf8'), {
    runScripts: 'dangerously',
    beforeParse(window) { Object.assign(window, { [bridge]: api }) },
  })
  pages.push(dom)
  const document = dom.window.document
  const visible = (element: Element): boolean => {
    for (let node: Element | null = element; node; node = node.parentElement) {
      if ((node as HTMLElement).hidden) return false
    }
    return true
  }
  return {
    calls,
    document,
    show: (state) => listener(state),
    visibleText: () => [...document.querySelectorAll('h1, h2, p, pre, button, label:not(.choice), .choice-title, .choice-detail')]
      .filter(visible)
      .map((element) => element.textContent?.trim() ?? '')
      .filter((text) => text !== '')
      .join('\n'),
    button: (label) => [...document.querySelectorAll('button')]
      .find((button) => visible(button) && button.textContent?.trim() === label),
  }
}

function openSetup(): Page<SetupState> {
  return openPage('setup.html', 'pagisSetup', ['installHere', 'connectToServer', 'upgrade', 'cancel', 'usePort', 'retryServer', 'quit'])
}

function openStatus(): Page<DaemonState> {
  return openPage('status.html', 'pagisStatus', ['retry', 'usePort', 'revealLogs', 'quit'])
}

/** Select one of the choices of a screen by its title. */
function choose(page: Page<unknown>, title: string): void {
  const choice = [...page.document.querySelectorAll('label.choice')]
    .find((label) => label.querySelector('.choice-title')?.textContent?.trim() === title)
  if (!choice) throw new Error(`no choice "${title}"`)
  const input = choice.querySelector('input')!
  input.checked = true
  input.dispatchEvent(new page.document.defaultView!.Event('change', { bubbles: true }))
}

/** The labels of the visible buttons of the footer, from left to right. */
function footer(page: Page<unknown>): string[] {
  return [...page.document.querySelectorAll('.footer button')]
    .filter((button) => page.button(button.textContent?.trim() ?? '') === button)
    .map((button) => button.textContent?.trim() ?? '')
}

/** Type a server address, then press Continue. */
function connectTo(page: Page<SetupState>, address: string): void {
  choose(page, 'Connect to a Pagis server')
  ;(page.document.getElementById('server-url') as HTMLInputElement).value = address
  page.button('Continue')!.click()
}

/** The message that the page shows under the Server address field, or
 *  null when it shows none. A screen reader reads it as a description of
 *  the field, together with the trust line. */
function addressMessage(page: Page<SetupState>): string | null {
  const input = page.document.getElementById('server-url')!
  const described = (input.getAttribute('aria-describedby') ?? '').split(' ').filter(Boolean)
  expect(described, 'the field does not keep the trust line as its description').toContain('connect-trust')
  const message = page.document.getElementById('server-url-error')!
  if (input.getAttribute('aria-invalid') !== 'true') {
    expect(described).not.toContain('server-url-error')
    expect(message.hidden).toBe(true)
    return null
  }
  expect(described).toContain('server-url-error')
  expect(message.hidden, 'the message of the field is not visible').toBe(false)
  return message.textContent
}

/** A document of the repository as one line of prose: no bold marks,
 *  and one space for each run of white space. */
function prose(file: string): string {
  return fs.readFileSync(path.join(__dirname, '..', '..', file), 'utf8').replace(/\*\*/g, '').replace(/\s+/g, ' ')
}

describe('the first screen of the setup page', () => {
  it('asks how to use Pagis, with two choices and Quit and Continue in the footer', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })

    const text = page.visibleText()
    expect(text.split('\n')[1]).toBe('How do you want to use Pagis?')
    expect(text).toContain('Install on this computer')
    expect(text).toContain('Connect to a Pagis server')
    expect(footer(page)).toEqual(['Quit', 'Continue'])
    const radios = [...page.document.querySelectorAll<HTMLInputElement>('input[name="use"]')]
    expect(radios.filter((radio) => radio.checked)).toHaveLength(1)
  })

  /** A connected Client App registers this computer as a Host, and it
   *  runs each command that the server sends with no check of its own
   *  (ADR-0015). The Person reads one line about it under the field
   *  where they type the address. */
  it('shows the Server address field and the trust line only for "Connect to a Pagis server"', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })

    choose(page, 'Install on this computer')
    expect(page.visibleText()).not.toMatch(/Server address|trust/)

    choose(page, 'Connect to a Pagis server')
    const lines = page.visibleText().split('\n')
    const field = lines.indexOf('Server address')
    expect(field).toBeGreaterThan(lines.indexOf('Connect to a Pagis server'))
    expect(lines[field + 1]).toBe('Connect only to a server you trust.')
    expect(page.document.getElementById('connect-trust')?.closest('.field')).not.toBeNull()

    choose(page, 'Install on this computer')
    expect(page.visibleText()).not.toMatch(/Server address|trust/)
  })

  /** The setup page states the trust in one line. The Connect to a
   *  server page of the documentation site states it in full and quotes
   *  the line of the page. */
  it('keeps the full trust statement in the documentation, which quotes the line of the page', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })
    choose(page, 'Connect to a Pagis server')

    const line = page.document.getElementById('connect-trust')?.textContent?.trim() ?? ''

    for (const readme of ['docs-site/content/client-app/connect-to-a-server.mdx']) {
      const text = prose(readme)
      expect(text, readme).toContain('The server and its Administrator can then run commands on this computer, with the same access as you.')
      expect(text.includes(`"${line}"`), `${readme} does not quote the line of the page: ${line}`).toBe(true)
    }
  })

  /** The Person signs in on the server's own page in the product window,
   *  so the setup page asks for no email address and no password. */
  it('collects no email address and no password', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })
    choose(page, 'Connect to a Pagis server')

    expect(page.document.querySelectorAll('input[type="email"], input[type="password"]')).toHaveLength(0)
    expect(page.visibleText()).not.toMatch(/email|password/i)
  })

  it('sends the address alone on Continue', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })

    connectTo(page, 'https://pagis.example.com')

    expect(addressMessage(page)).toBeNull()
    expect(page.calls).toEqual(['connectToServer:https://pagis.example.com'])
  })

  it('sends the address when the person presses Enter in the field', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })
    choose(page, 'Connect to a Pagis server')
    const input = page.document.getElementById('server-url') as HTMLInputElement
    input.value = 'pagis.example.com'

    input.dispatchEvent(new page.document.defaultView!.KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))

    expect(page.calls).toEqual(['connectToServer:pagis.example.com'])
  })

  it('names an empty address on its field, sends nothing and focuses the field', () => {
    for (const address of ['', '   ']) {
      const page = openSetup()
      page.show({ kind: 'ready' })

      connectTo(page, address)

      expect(page.calls).toEqual([])
      expect(addressMessage(page)).toBe('Enter the address of your Pagis server.')
      expect(page.document.activeElement?.id).toBe('server-url')
    }
  })

  /** The main process reads the address with serverOrigin and refuses
   *  it with the same words, so the page and the main process never
   *  disagree on an address. */
  it('names a malformed server address on its field, in the words of the main process', () => {
    for (const url of [
      'ada@example.com',
      'pagis example.com',
      'https://',
      'ftp://pagis.example.com',
      'https://ada:secret@pagis.example.com',
      'http://pagis.example.com',
    ]) {
      const page = openSetup()
      page.show({ kind: 'ready' })

      connectTo(page, url)

      expect(page.calls, url).toEqual([])
      let refusal = ''
      try { serverOrigin(url) } catch (error) { refusal = (error as Error).message }
      expect(refusal, url).not.toBe('')
      expect(addressMessage(page), url).toBe(refusal)
      expect(page.document.activeElement?.id, url).toBe('server-url')
    }
  })

  it('clears the message when the person corrects the address, and sends it', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })
    connectTo(page, 'ada@example.com')

    connectTo(page, 'https://pagis.example.com')

    expect(addressMessage(page)).toBeNull()
    expect(page.calls).toEqual(['connectToServer:https://pagis.example.com'])
  })

  /** A server that does not answer, one outside the Compatibility Range,
   *  and one with no administrator yet are problems with the address that
   *  the person typed. The window stays on the first screen. */
  it('shows a failed server check under the field, and keeps the first screen', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })
    connectTo(page, 'pagis.example.com')
    page.show({ kind: 'installing', detail: 'Connecting to the Pagis server…' })

    expect(page.visibleText().split('\n')[1]).toBe('How do you want to use Pagis?')
    expect(page.visibleText()).toContain('Connecting to the Pagis server…')
    expect(page.document.querySelector<HTMLButtonElement>('#continue')?.disabled).toBe(true)

    const reason = 'This Pagis server has no administrator yet. Finish its setup on the administration page at http://127.0.0.1:4701/ on the server itself, then connect again.'
    page.show({ kind: 'server-check-failed', reason })

    expect(page.visibleText().split('\n')[1]).toBe('How do you want to use Pagis?')
    expect(addressMessage(page)).toBe(reason)
    expect((page.document.getElementById('server-url') as HTMLInputElement).value).toBe('pagis.example.com')
    expect(page.document.activeElement?.id).toBe('server-url')
    expect(page.button('Continue')?.disabled).toBe(false)
    expect(page.button('Repair')).toBeUndefined()
    expect(footer(page)).toEqual(['Quit', 'Continue'])
  })
})

describe('the second screen of the setup page', () => {
  it('asks who uses Pagis, with Back and Install in the footer', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })

    page.button('Continue')!.click()

    const text = page.visibleText()
    expect(text.split('\n')[1]).toBe('Who uses Pagis on this computer?')
    expect(text).toContain('Just me')
    expect(text).toContain('Several people')
    expect(text).not.toContain('Server address')
    expect(footer(page)).toEqual(['Quit', 'Back', 'Install'])
  })

  it('installs for the answer that the person selected', () => {
    for (const [answer, people] of [['Just me', 'one'], ['Several people', 'several']] as const) {
      const page = openSetup()
      page.show({ kind: 'ready' })
      page.button('Continue')!.click()

      choose(page, answer)
      page.button('Install')!.click()

      expect(page.calls).toEqual([`installHere:${people}`])
    }
  })

  it('goes back to the first screen', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })
    page.button('Continue')!.click()

    page.button('Back')!.click()

    expect(page.visibleText().split('\n')[1]).toBe('How do you want to use Pagis?')
  })

  it('shows the progress after Install, with Cancel', () => {
    const page = openSetup()
    page.show({ kind: 'ready' })
    page.button('Continue')!.click()
    page.button('Install')!.click()

    page.show({ kind: 'setting-up', step: 'download', download: { fraction: 0.25, detail: '18 of 72 MB · About 20 seconds left' }, upgrade: null })

    expect(page.visibleText()).toContain('18 of 72 MB · About 20 seconds left')
    expect(footer(page)).toEqual(['Quit', 'Cancel'])
    page.button('Cancel')!.click()
    expect(page.calls).toEqual(['installHere:one', 'cancel'])
  })
})

describe('the states of the setup page', () => {
  it('shows a taken port with the process and the next free port, and no setup question', () => {
    const page = openSetup()

    page.show({ kind: 'taken-port', port: 4410, holder: 'Python (pid 82674)', suggested: 4411 })

    expect(page.visibleText()).toContain('Python (pid 82674) uses port 4410. Pagis can use port 4411.')
    expect(page.visibleText()).not.toMatch(/Who uses Pagis|How do you want/)
    expect(footer(page)).toEqual(['Quit', 'Use port 4411'])
    page.button('Use port 4411')!.click()
    expect(page.calls).toEqual(['usePort:4411'])
  })

  it('repairs an installation without asking who uses Pagis', () => {
    const page = openSetup()

    page.show({ kind: 'failed', reason: 'The daemon stopped 4 times with code 1.', repair: true, log: '' })

    expect(page.visibleText()).toContain('The daemon stopped 4 times with code 1.')
    expect(footer(page)).toEqual(['Quit', 'Choose another setup', 'Repair'])
    page.button('Repair')!.click()
    expect(page.calls).toEqual(['installHere:one'])
    expect(page.visibleText()).not.toContain('Who uses Pagis')
  })

  it('shows what the stopped server printed under the reason', () => {
    const page = openSetup()
    const log = 'Error: migration 2 was previously applied but is missing in the resolved migrations'

    page.show({ kind: 'failed', reason: 'The daemon stopped 4 times with code 1.', repair: true, log })

    expect(page.visibleText().split('\n').slice(1, 4))
      .toEqual(['Pagis could not start', 'The daemon stopped 4 times with code 1.', log])
  })

  it('shows no log for a failure without one', () => {
    const page = openSetup()

    page.show({ kind: 'failed', reason: 'server download failed with HTTP 404', repair: false, log: '' })

    expect(page.document.getElementById('log')!.hidden).toBe(true)
  })

  it('offers the other setups after a failure on request', () => {
    const page = openSetup()
    page.show({ kind: 'failed', reason: 'The daemon stopped 4 times with code 1.', repair: true, log: '' })

    page.button('Choose another setup')!.click()

    expect(page.visibleText().split('\n')[1]).toBe('How do you want to use Pagis?')
    expect(footer(page)).toEqual(['Quit', 'Continue'])
  })

  it('offers the setups again after a failure of a new setup, and no Repair', () => {
    const page = openSetup()

    page.show({ kind: 'failed', reason: 'server download failed with HTTP 404', repair: false, log: '' })

    expect(page.visibleText()).toContain('server download failed with HTTP 404')
    expect(page.button('Repair')).toBeUndefined()
    page.button('Choose another setup')!.click()
    expect(page.visibleText().split('\n')[1]).toBe('How do you want to use Pagis?')
  })

  /** A connected client whose server does not answer says so, and offers
   *  to try the server again or another setup. There is nothing on this
   *  computer to repair. */
  it('offers to try a connected server again or another setup, and no Repair', () => {
    const page = openSetup()
    const reason = 'No Pagis server answered at http://127.0.0.1:4700/. Check the address and that the server is running.'

    page.show({ kind: 'connection-failed', origin: 'http://127.0.0.1:4700', reason })

    expect(page.visibleText()).toContain(reason)
    expect(footer(page)).toEqual(['Quit', 'Choose another setup', 'Try again'])
    page.button('Try again')!.click()
    expect(page.calls).toEqual(['retryServer'])
    page.button('Choose another setup')!.click()
    expect(page.button('Continue')).toBeDefined()
  })

  /** A failed Backup stops the Upgrade before the data changes. The
   *  Person tries the Backup again, or continues without one. */
  it('offers Retry and "Continue without a Backup" after a failed Backup, and no other setup', () => {
    const page = openSetup()
    const reason = 'The Backup of Pagis 0.1.1 did not complete: pagis is running against /Users/ada/.pagis — stop the daemon first'

    page.show({ kind: 'backup-failed', release: '0.2.0', reason })

    expect(page.visibleText().split('\n').slice(1, 4)).toEqual([
      'Pagis could not make a Backup',
      'Pagis stopped the Upgrade to 0.2.0 before it changed your data. Select Retry, or continue without a Backup.',
      reason,
    ])
    expect(footer(page)).toEqual(['Quit', 'Continue without a Backup', 'Retry'])
    page.button('Retry')!.click()
    page.button('Continue without a Backup')!.click()
    expect(page.calls).toEqual(['upgrade:true', 'upgrade:false'])
  })

  it('gives each state a title of its own, under the name of the product', () => {
    for (const [state, title] of [
      [{ kind: 'setting-up', step: 'check', download: null, upgrade: null }, 'Setting up Pagis'],
      [{ kind: 'installing', detail: 'Connecting to the Pagis server…' }, 'Setting up Pagis'],
      [{ kind: 'taken-port', port: 4410, holder: 'Python (pid 82674)', suggested: 4411 }, 'Port 4410 is taken'],
      [{ kind: 'failed', reason: 'The daemon stopped.', repair: true, log: '' }, 'Pagis could not start'],
      [{ kind: 'failed', reason: 'The download stopped.', repair: false, log: '' }, 'Setup did not complete'],
      [{ kind: 'connection-failed', origin: 'http://127.0.0.1:4700', reason: 'No answer.' }, 'Pagis cannot open http://127.0.0.1:4700'],
    ] as const) {
      const page = openSetup()

      page.show(state)

      expect(page.visibleText().split('\n').slice(0, 2)).toEqual(['Pagis', title])
    }
  })
})

/** A local setup lists its steps, with a bar for the current one. Only
 *  the download has a share to show; the other steps show a busy bar. */
describe('the steps of a local setup', () => {
  const steps = (page: Page<SetupState>): string[] =>
    [...page.document.querySelectorAll<HTMLElement>('#steps li')]
      .filter((step) => !step.hidden)
      .map((step) => `${step.dataset.state}:${step.textContent?.trim()}`)
  const bar = (page: Page<SetupState>): HTMLElement => page.document.getElementById('setup-bar')!

  it('lists the four steps, with the finished ones done and the current one marked', () => {
    const page = openSetup()

    page.show({ kind: 'setting-up', step: 'install', download: null, upgrade: null })

    expect(steps(page)).toEqual([
      'done:Download the server',
      'done:Check the server',
      'current:Install the server files',
      'pending:Start the server',
    ])
  })

  it('fills the bar with the share of the download', () => {
    const page = openSetup()

    page.show({ kind: 'setting-up', step: 'download', download: { fraction: 0.25, detail: '18 of 72 MB' }, upgrade: null })

    expect(bar(page).getAttribute('aria-valuenow')).toBe('25')
    expect((bar(page).firstElementChild as HTMLElement).style.width).toBe('25%')
    expect(page.visibleText()).toContain('18 of 72 MB')
  })

  it('shows a busy bar and no detail while a step has no share', () => {
    const page = openSetup()
    page.show({ kind: 'setting-up', step: 'download', download: { fraction: 0.25, detail: '18 of 72 MB' }, upgrade: null })

    page.show({ kind: 'setting-up', step: 'start', download: null, upgrade: null })

    expect(bar(page).hasAttribute('aria-valuenow')).toBe(false)
    expect(bar(page).firstElementChild!.classList.contains('progress-busy')).toBe(true)
    expect(page.visibleText()).not.toContain('18 of 72 MB')
  })

  /** An Upgrade asks no setup question. Its heading names the new
   *  release, and its Backup comes before the other steps (ADR-0027). */
  it('names the Upgrade to the new release, and lists its Backup first', () => {
    const page = openSetup()

    page.show({ kind: 'setting-up', step: 'download', download: null, upgrade: { release: '0.2.0', backup: true } })

    expect(page.visibleText().split('\n').slice(0, 2)).toEqual(['Pagis', 'Upgrading Pagis to 0.2.0'])
    expect(steps(page)).toEqual([
      'done:Make a Backup of your data',
      'current:Download the server',
      'pending:Check the server',
      'pending:Install the server files',
      'pending:Start the server',
    ])
    expect(page.visibleText()).not.toMatch(/Who uses Pagis|How do you want/)
  })

  it('lists no Backup in an Upgrade that continues without one', () => {
    const page = openSetup()

    page.show({ kind: 'setting-up', step: 'check', download: null, upgrade: { release: '0.2.0', backup: false } })

    expect(steps(page)).toEqual([
      'done:Download the server',
      'current:Check the server',
      'pending:Install the server files',
      'pending:Start the server',
    ])
  })

  /** Cancel goes back to the setup question, and an Upgrade has none.
   *  Quit stops it. */
  it('offers no Cancel during an Upgrade', () => {
    const page = openSetup()

    page.show({ kind: 'setting-up', step: 'backup', download: null, upgrade: { release: '0.2.0', backup: true } })

    expect(footer(page)).toEqual(['Quit'])
  })

  it('shows the steps of a local setup only', () => {
    const page = openSetup()

    page.show({ kind: 'installing', detail: 'Connecting to the Pagis server…' })

    expect(page.document.getElementById('steps')!.hidden).toBe(true)
    expect(page.visibleText()).toContain('Connecting to the Pagis server…')
  })
})

/** Quit ends the Client App from each screen of the setup. */
describe('Quit on the setup page', () => {
  it('asks the main process to quit from each screen and state', () => {
    const screens: [string, (page: Page<SetupState>) => void][] = [
      ['the first screen', (page) => page.show({ kind: 'ready' })],
      ['the second screen', (page) => { page.show({ kind: 'ready' }); page.button('Continue')!.click() }],
      ['the progress', (page) => page.show({ kind: 'installing', detail: 'Starting the Pagis server…' })],
      ['a failure', (page) => page.show({ kind: 'failed', reason: 'x', repair: true, log: '' })],
      ['a taken port', (page) => page.show({ kind: 'taken-port', port: 4410, holder: 'Python', suggested: 4411 })],
      ['an Upgrade', (page) => page.show({ kind: 'setting-up', step: 'backup', download: null, upgrade: { release: '0.2.0', backup: true } })],
      ['a failed Backup', (page) => page.show({ kind: 'backup-failed', release: '0.2.0', reason: 'x' })],
    ]
    for (const [name, open] of screens) {
      const page = openSetup()
      open(page)

      page.button('Quit')!.click()

      expect(page.calls, name).toEqual(['quit'])
    }
  })
})

describe('the status page', () => {
  it('shows a taken port with its next action first', () => {
    const page = openStatus()

    page.show({ kind: 'taken-port', port: 4410, holder: 'Python (pid 82674)', suggested: 4411 })

    const lines = page.visibleText().split('\n')
    expect(lines.slice(1, 4)).toEqual([
      'Port 4410 is taken',
      'Python (pid 82674) listens on port 4410, so Pagis cannot start there. Pagis can move to port 4411.',
      'Use port 4411',
    ])
    page.button('Use port 4411')!.click()
    expect(page.calls).toEqual(['usePort:4411'])
  })

  it('shows a failure with its reason, the log and Retry', () => {
    const page = openStatus()

    page.show({ kind: 'failed', reason: 'The daemon stopped 4 times with code 1.', log: 'the boot failed' })

    const lines = page.visibleText().split('\n')
    expect(lines.slice(1, 5)).toEqual(['Pagis could not start', 'The daemon stopped 4 times with code 1.', 'the boot failed', 'Retry'])
    page.button('Retry')!.click()
    page.button('Show the log')!.click()
    expect(page.calls).toEqual(['retry', 'revealLogs'])
  })

  it('says that Pagis is starting before a state arrives', () => {
    const page = openStatus()

    expect(page.visibleText()).toContain('Pagis is starting…')
  })
})
