// The multi-user mode control: one switch that shows the mode, the form
// that turns it on, the confirmation that turns it off, and the restart
// that puts either one in effect.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, MultiUserDto, ScreenDto } from '../../api/client'
import { MULTI_USER_ANCHOR, MultiUser, SCREEN_GUIDE, SETUP_GUIDE } from './MultiUser'

const off: MultiUserDto = {
  enabled: false,
  public_origin: null,
  trusted_proxy: null,
  switchable: true,
}

const on: MultiUserDto = {
  enabled: true,
  public_origin: 'https://pagis.owner.example',
  trusted_proxy: '127.0.0.1',
  switchable: true,
}

/** A local installation that configures no `[screen]` section. */
const loopbackScreen: ScreenDto = {
  relay: 'daemon',
  advertise_ip: '127.0.0.1',
  loopback: true,
  media_port_first: 50000,
  media_port_last: 50099,
}

const lanScreen: ScreenDto = { ...loopbackScreen, advertise_ip: '192.168.86.55', loopback: false }

function saved(multiUser: MultiUserDto, restartRequired = true) {
  return { data: { settings: { multi_user: multiUser }, restart_required: restartRequired } }
}

function stubApi(overrides: Partial<Record<'GET' | 'PUT' | 'DELETE' | 'POST', unknown>> = {}) {
  return {
    // The same process answers: no restart has happened yet.
    GET: vi.fn(async () => ({ data: { started_at: 1 } })),
    PUT: vi.fn(async () => saved(on)),
    DELETE: vi.fn(async () => saved(off)),
    POST: vi.fn(async () => ({ data: { exit_code: 75 } })),
    ...overrides,
  }
}

function mount(
  api: ReturnType<typeof stubApi>,
  multiUser: MultiUserDto,
  supervised = true,
  screenRelay: ScreenDto = loopbackScreen,
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <MultiUser
        api={api as unknown as ApiClient}
        multiUser={multiUser}
        screen={screenRelay}
        daemon={{ supervised, started_at: 1 }}
      />
    </QueryClientProvider>,
  )
}

function modeSwitch() {
  return screen.getByRole('switch', { name: /Multi-user mode/ })
}

describe('MultiUser', () => {
  it('scrolls to the switch when the address names it', () => {
    const scrolled = vi.fn()
    Element.prototype.scrollIntoView = scrolled
    window.history.replaceState(null, '', `/settings#${MULTI_USER_ANCHOR}`)
    try {
      mount(stubApi(), off)

      expect(document.getElementById(MULTI_USER_ANCHOR)?.contains(modeSwitch())).toBe(true)
      expect(scrolled).toHaveBeenCalledTimes(1)
    } finally {
      window.history.replaceState(null, '', '/')
    }
  })

  it('does not scroll when the address names no switch', () => {
    const scrolled = vi.fn()
    Element.prototype.scrollIntoView = scrolled

    mount(stubApi(), off)

    expect(scrolled).not.toHaveBeenCalled()
  })

  it('shows the mode off on a local installation', () => {
    mount(stubApi(), off)

    expect(modeSwitch().getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText('Only this computer reaches Pagis.')).toBeTruthy()
    expect(screen.queryByLabelText('Public Origin')).toBeNull()
  })

  it('asks for the origin and the proxy, and says who provides TLS', () => {
    mount(stubApi(), off)

    fireEvent.click(modeSwitch())

    expect(modeSwitch().getAttribute('aria-checked')).toBe('true')
    expect((screen.getByLabelText('Public Origin') as HTMLInputElement).value).toBe('')
    expect((screen.getByLabelText('Trusted Proxy') as HTMLInputElement).value).toBe(
      '127.0.0.1',
    )
    expect(screen.getByText(/Pagis does not provide TLS/)).toBeTruthy()
    const guide = screen.getByRole('link', { name: /Caddy, Tailscale Serve or Cloudflare Tunnel/ })
    expect(guide.getAttribute('href')).toBe(SETUP_GUIDE)
    expect(
      (screen.getByRole('button', { name: 'Turn on and restart' }) as HTMLButtonElement).disabled,
    ).toBe(true)
  })

  it('turns the mode on and restarts', async () => {
    const api = stubApi()
    mount(api, off)

    fireEvent.click(modeSwitch())
    fireEvent.change(screen.getByLabelText('Public Origin'), {
      target: { value: ' https://pagis.owner.example ' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Turn on and restart' }))

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/system/multi-user', {
        body: { public_origin: 'https://pagis.owner.example', trusted_proxy: '127.0.0.1' },
      }),
    )
    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/system/restart'))
    expect(await screen.findByText('Pagis is starting again.')).toBeTruthy()
  })

  it('says the live screen stays on this computer, and links to its setup', () => {
    mount(stubApi(), off)
    fireEvent.click(modeSwitch())

    expect(screen.getByText(/does not go through your proxy or tunnel/)).toBeTruthy()
    expect(
      screen.getByRole('link', { name: 'Set up the live screen for other people' }).getAttribute('href'),
    ).toBe(SCREEN_GUIDE)
  })

  it('keeps the live screen note while the mode is on', () => {
    mount(stubApi(), on)

    expect(screen.getByText(/does not go through your proxy or tunnel/)).toBeTruthy()
  })

  it('names a loopback address, says only this computer reaches it, and how to change it', () => {
    mount(stubApi(), on, true, loopbackScreen)

    const note = screen.getByText(/does not go through your proxy or tunnel/)
    expect(note.textContent).toContain('The Media Relay advertises 127.0.0.1')
    expect(note.textContent).toContain('only this computer reaches the live screen')
    expect(note.textContent).toContain('set [screen] advertise_ip in config.toml')
    expect(note.textContent).toContain('restart Pagis')
    expect(note.textContent).not.toContain('UDP')
  })

  it('names a LAN address, the machines that reach it, and the UDP range to open', () => {
    mount(stubApi(), on, true, lanScreen)

    const note = screen.getByText(/does not go through your proxy or tunnel/)
    expect(note.textContent).toContain('The Media Relay advertises 192.168.86.55')
    expect(note.textContent).toContain('machines that reach this address reach the live screen')
    expect(note.textContent).toContain('UDP ports 50000–50099')
    expect(note.textContent).not.toContain('only this computer')
  })

  it('says a TURN relay carries the live screen to the machines that reach it', () => {
    mount(stubApi(), on, true, { ...loopbackScreen, relay: 'turn' })

    const note = screen.getByText(/goes through your TURN server/)
    expect(note.textContent).toContain('Machines that reach the TURN server reach the live screen')
    expect(note.textContent).not.toContain('only this computer')
  })

  it('sends no proxy when the field is empty', async () => {
    const api = stubApi()
    mount(api, off)

    fireEvent.click(modeSwitch())
    fireEvent.change(screen.getByLabelText('Public Origin'), {
      target: { value: 'https://pagis.owner.example' },
    })
    fireEvent.change(screen.getByLabelText('Trusted Proxy'), { target: { value: ' ' } })
    fireEvent.click(screen.getByRole('button', { name: 'Turn on and restart' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    expect((api.PUT as ReturnType<typeof vi.fn>).mock.calls[0][1].body.trusted_proxy).toBe(null)
  })

  it('shows the reason when the daemon refuses the origin, and does not restart', async () => {
    const api = stubApi({
      PUT: vi.fn(async () => ({
        error: {
          error: {
            code: 'validation',
            message: 'http://localhost:4400 is not an address people on other machines can open',
          },
        },
      })),
    })
    mount(api, off)

    fireEvent.click(modeSwitch())
    fireEvent.change(screen.getByLabelText('Public Origin'), {
      target: { value: 'http://localhost:4400' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Turn on and restart' }))

    expect(
      await screen.findByText(
        'http://localhost:4400 is not an address people on other machines can open',
      ),
    ).toBeTruthy()
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('cancels back to the mode as it is, with no call', () => {
    const api = stubApi()
    mount(api, off)

    fireEvent.click(modeSwitch())
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    expect(modeSwitch().getAttribute('aria-checked')).toBe('false')
    expect(screen.queryByLabelText('Public Origin')).toBeNull()
    expect(api.PUT).not.toHaveBeenCalled()
  })

  it('shows the origin people open while the mode is on', () => {
    // A LAN Media Relay, so 127.0.0.1 is the Trusted Proxy alone.
    mount(stubApi(), on, true, lanScreen)

    expect(modeSwitch().getAttribute('aria-checked')).toBe('true')
    expect(screen.getByText('https://pagis.owner.example')).toBeTruthy()
    expect(screen.getByText('127.0.0.1')).toBeTruthy()
  })

  it('says what turning it off does, then turns it off and restarts', async () => {
    const api = stubApi()
    mount(api, on)

    fireEvent.click(modeSwitch())

    expect(modeSwitch().getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText(/keep their accounts/)).toBeTruthy()
    expect(api.DELETE).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Turn off and restart' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/system/multi-user'),
    )
    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/system/restart'))
  })

  it('does not restart when the daemon already runs in the mode it saved', async () => {
    const api = stubApi({ DELETE: vi.fn(async () => saved(off, false)) })
    mount(api, on)

    fireEvent.click(modeSwitch())
    fireEvent.click(screen.getByRole('button', { name: 'Turn off and restart' }))

    expect(await screen.findByText('Saved.')).toBeTruthy()
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('shows a server as always multi-user, with no switch', () => {
    mount(stubApi(), { ...on, trusted_proxy: null, switchable: false })

    expect(screen.queryByRole('switch')).toBeNull()
    expect(screen.getByText('https://pagis.owner.example')).toBeTruthy()
    expect(screen.getByText(/A server always serves several People/)).toBeTruthy()
  })

  it('names the Media Relay of a server too', () => {
    mount(stubApi(), { ...on, switchable: false }, true, lanScreen)

    expect(screen.getByText(/does not go through your proxy or tunnel/).textContent).toContain(
      'The Media Relay advertises 192.168.86.55',
    )
  })
})
