// The Remote Access switch: each state of Tailscale with its one action,
// the page that Tailscale names while a turn-on waits, the public name
// when it is on, and the restart that puts either change in effect.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient, RemoteAccessDto, ScreenDto, TailscaleState } from '../../api/client'
import { REMOTE_ACCESS_ANCHOR, RemoteAccess } from './RemoteAccess'

const NAME = 'owner-mac.tail1234.ts.net'
const ORIGIN = `https://${NAME}`
const ENABLE_URL = 'https://login.tailscale.com/f/funnel?node=nTEST000000CNTRL'

const ready: TailscaleState = { state: 'ready', dns_name: NAME, port_443: { serves: 'nothing' } }

function off(tailscale: TailscaleState = ready): RemoteAccessDto {
  return {
    enabled: false,
    restart_required: false,
    public_origin: null,
    switchable: true,
    tailscale,
    turning_on: null,
    failure: null,
  }
}

function on(overrides: Partial<RemoteAccessDto> = {}): RemoteAccessDto {
  return {
    ...off({ state: 'ready', dns_name: NAME, port_443: { serves: 'pagis' } }),
    enabled: true,
    public_origin: ORIGIN,
    ...overrides,
  }
}

/** A local installation that configures no `[screen]` section. */
const loopbackScreen: ScreenDto = {
  relay: 'daemon',
  advertise_ip: '127.0.0.1',
  loopback: true,
  media_port_first: 50000,
  media_port_last: 50099,
}

/** An API whose reads of Remote Access answer `reads` in turn, the last
 *  one again and again. A read of the System Settings answers the same
 *  process, so no restart has happened yet. */
function stubApi(
  reads: RemoteAccessDto[],
  overrides: Partial<Record<'PUT' | 'DELETE' | 'POST', unknown>> = {},
) {
  let next = 0
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/settings/system') return { data: { started_at: 1 } }
      const read = reads[Math.min(next, reads.length - 1)]
      next += 1
      return { data: read }
    }),
    PUT: vi.fn(async () => ({ data: { ...off(), turning_on: { enable_url: null } } })),
    DELETE: vi.fn(async () => ({ data: off() })),
    POST: vi.fn(async () => ({ data: { exit_code: 75 } })),
    ...overrides,
  }
}

function mount(api: ReturnType<typeof stubApi>, supervised = true) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <RemoteAccess
        api={api as unknown as ApiClient}
        screen={loopbackScreen}
        daemon={{ supervised, started_at: 1 }}
      />
    </QueryClientProvider>,
  )
}

async function theSwitch() {
  return screen.findByRole('switch', { name: /Remote Access/ })
}

afterEach(() => {
  vi.useRealTimers()
  window.history.replaceState(null, '', '/')
})

describe('RemoteAccess', () => {
  it('scrolls to the switch when the address names it', async () => {
    const scrolled = vi.fn()
    Element.prototype.scrollIntoView = scrolled
    window.history.replaceState(null, '', `/settings#${REMOTE_ACCESS_ANCHOR}`)

    mount(stubApi([off()]))

    expect(document.getElementById(REMOTE_ACCESS_ANCHOR)?.contains(await theSwitch())).toBe(true)
    await waitFor(() => expect(scrolled).toHaveBeenCalledTimes(1))
  })

  it('names Tailscale to install, with no switch to turn', async () => {
    const api = stubApi([
      off({ state: 'not_installed', install_url: 'https://pkgs.tailscale.com/stable/#macos' }),
    ])
    mount(api)

    expect((await theSwitch()).hasAttribute('disabled')).toBe(true)
    expect(screen.getByText(/Tailscale is not on this computer/)).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Tailscale' }).getAttribute('href')).toBe(
      'https://pkgs.tailscale.com/stable/#macos',
    )

    fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
    await waitFor(() => expect(api.GET).toHaveBeenCalledTimes(2))
  })

  it('says to open Tailscale and sign in, with what Tailscale said', async () => {
    mount(stubApi([off({ state: 'not_running', detail: 'is Tailscale running?' })]))

    expect((await theSwitch()).hasAttribute('disabled')).toBe(true)
    expect(screen.getByText(/Open Tailscale and\s+sign in/)).toBeTruthy()
    expect(screen.getByText('is Tailscale running?')).toBeTruthy()
  })

  it('names what port 443 serves and does not replace it', async () => {
    mount(
      stubApi([
        off({
          state: 'ready',
          dns_name: NAME,
          port_443: { serves: 'other', target: '/ http://127.0.0.1:3000' },
        }),
      ]),
    )

    expect((await theSwitch()).hasAttribute('disabled')).toBe(true)
    expect(screen.getByText('/ http://127.0.0.1:3000')).toBeTruthy()
    expect(screen.getByText('tailscale funnel --https=443 off')).toBeTruthy()
  })

  it('says that Tailscale is ready and names the public name before a turn-on', async () => {
    mount(stubApi([off()]))

    const toggle = await theSwitch()
    expect(toggle.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText('Only this computer reaches Pagis.')).toBeTruthy()
    expect(screen.getByText(/Tailscale is ready/)).toBeTruthy()

    fireEvent.click(toggle)

    expect(toggle.getAttribute('aria-checked')).toBe('true')
    expect(screen.getByText(/never with a password/)).toBeTruthy()
    expect(screen.getAllByText(ORIGIN).length).toBeGreaterThan(0)
    expect(screen.getByRole('button', { name: 'Turn on and restart' })).toBeTruthy()
  })

  it('turns on, shows the page that Tailscale names, and restarts when the turn-on ends', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const api = stubApi([
      off({ state: 'funnel_off', port_443: { serves: 'nothing' } }),
      { ...off(), turning_on: { enable_url: ENABLE_URL } },
      { ...on(), restart_required: true },
    ])
    mount(api)
    expect(await screen.findByText(/HTTPS and Funnel are off for your tailnet/)).toBeTruthy()

    fireEvent.click(await theSwitch())
    fireEvent.click(screen.getByRole('button', { name: 'Turn on and restart' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/system/remote-access'))
    expect(await screen.findByText('Tailscale turns on Funnel…')).toBeTruthy()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_100)
    })
    expect((await screen.findByRole('link', { name: ENABLE_URL })).getAttribute('href')).toBe(
      ENABLE_URL,
    )
    // Nothing restarts while Tailscale waits for the approval.
    expect(api.POST).not.toHaveBeenCalled()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_100)
    })

    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/system/restart'))
    expect(await screen.findByText('Pagis is starting again.')).toBeTruthy()
  })

  it('stops a turn-on that waits', async () => {
    const api = stubApi([{ ...off(), turning_on: { enable_url: ENABLE_URL } }, off()])
    mount(api)
    expect(await screen.findByRole('link', { name: ENABLE_URL })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/system/remote-access'),
    )
    expect(await screen.findByText('Only this computer reaches Pagis.')).toBeTruthy()
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('shows why a turn-on failed', async () => {
    mount(stubApi([{ ...off(), failure: 'Funnel not available on this build' }]))

    expect((await screen.findByRole('alert')).textContent).toBe(
      'Funnel not available on this build',
    )
  })

  it('shows the public name when it is on, and what another machine sees of the live screen', async () => {
    mount(stubApi([on()]))

    expect((await theSwitch()).getAttribute('aria-checked')).toBe('true')
    expect(screen.getByRole('link', { name: ORIGIN }).getAttribute('href')).toBe(ORIGIN)
    expect(screen.getByText(/sign in with a sign-in link/)).toBeTruthy()
    expect(screen.getByText(/Live screen unavailable/)).toBeTruthy()
  })

  it('offers to turn on again where the Funnel does not serve Pagis', async () => {
    const api = stubApi([on({ tailscale: ready })])
    mount(api)

    expect((await screen.findByRole('alert')).textContent).toMatch(/does not serve Pagis now/)
    fireEvent.click(screen.getByRole('button', { name: 'Turn on again' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
  })

  it('turns off and restarts', async () => {
    const api = stubApi([on()], {
      DELETE: vi.fn(async () => ({ data: { ...off(), restart_required: true } })),
    })
    mount(api)

    fireEvent.click(await theSwitch())
    expect(screen.getByText(/keep their accounts and Sessions/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Turn off and restart' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/system/remote-access'),
    )
    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/system/restart'))
  })

  it('cancels a switch that it has not asked for', async () => {
    const api = stubApi([off()])
    mount(api)

    fireEvent.click(await theSwitch())
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    expect((await theSwitch()).getAttribute('aria-checked')).toBe('false')
    expect(api.PUT).not.toHaveBeenCalled()
  })

  it('shows a server with no switch', async () => {
    mount(
      stubApi([
        {
          ...off(),
          switchable: false,
          tailscale: null,
        },
      ]),
    )

    expect(await screen.findByText(/This Pagis is a server/)).toBeTruthy()
    expect(screen.getByText('PAGIS_REMOTE_ACCESS')).toBeTruthy()
    expect(screen.queryByRole('switch')).toBeNull()
  })
})
