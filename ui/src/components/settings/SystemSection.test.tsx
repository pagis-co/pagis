// The System section: the two-column daemon form, Save and
// restart, the Docker probe row and About.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { SystemSection } from './SystemSection'

const settings = {
  multi_user: {
    enabled: false,
    public_origin: null,
    trusted_proxy: null,
    switchable: true,
  },
  screen: {
    relay: 'daemon' as const,
    advertise_ip: '127.0.0.1',
    loopback: true,
    media_port_first: 50000,
    media_port_last: 50099,
  },
  port: 4400,
  listening_port: 4400,
  port_override: null,
  supervised: true,
  started_at: 1,
  docker_endpoint: null,
  log_level: 'info',
  data_directory: '/Users/ada/.pagis',
  version: '0.14.2',
  docker: {
    endpoint: 'unix:///Users/ada/.colima/default/docker.sock',
    candidates: [
      {
        source: 'docker_run',
        endpoint: 'unix:///Users/ada/.docker/run/docker.sock',
        reachable: false,
        error: 'connection refused',
      },
      {
        source: 'colima',
        endpoint: 'unix:///Users/ada/.colima/default/docker.sock',
        reachable: true,
        error: null,
      },
    ],
  },
}

function stubApi(overrides: Partial<Record<'GET' | 'PUT' | 'POST', unknown>> = {}) {
  return {
    GET: vi.fn(async () => ({ data: settings })),
    PUT: vi.fn(async () => ({
      data: { settings: { ...settings, port: 4500 }, restart_required: true },
    })),
    POST: vi.fn(async () => ({ data: settings.docker })),
    ...overrides,
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <SystemSection api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

describe('SystemSection', () => {
  it('shows the three fields and About', async () => {
    mount(stubApi())

    expect(((await screen.findByLabelText('Port')) as HTMLInputElement).value).toBe('4400')
    expect(screen.getByRole('combobox', { name: 'Log level' }).textContent).toContain('info')
    expect((screen.getByLabelText('Docker endpoint') as HTMLInputElement).value).toBe('')
    expect(screen.getByText('Data directory')).toBeTruthy()
    expect(screen.getByText('/Users/ada/.pagis')).toBeTruthy()
    expect(screen.getByText('Version')).toBeTruthy()
    expect(screen.getByText('pagis 0.14.2')).toBeTruthy()
  })

  it('shows the multi-user mode with its switch', async () => {
    mount(stubApi())

    expect(await screen.findByText('Network')).toBeTruthy()
    expect(
      screen.getByRole('switch', { name: /Multi-user mode/ }).getAttribute('aria-checked'),
    ).toBe('false')
  })

  it('names the Docker endpoint in use', async () => {
    mount(stubApi())

    expect(await screen.findByText('Reachable')).toBeTruthy()
    expect(
      screen.getByText('Colima · unix:///Users/ada/.colima/default/docker.sock'),
    ).toBeTruthy()
  })

  it('says when no Docker answered', async () => {
    mount(
      stubApi({
        GET: vi.fn(async () => ({
          data: { ...settings, docker: { endpoint: null, candidates: [] } },
        })),
      }),
    )

    expect(await screen.findByText('Unreachable')).toBeTruthy()
    expect(
      screen.getByText('Pagis found no Docker, so sprite computers cannot run.'),
    ).toBeTruthy()
  })

  it('saves the settings and restarts the daemon', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.change(await screen.findByLabelText('Port'), { target: { value: '4500' } })
    fireEvent.change(screen.getByLabelText('Docker endpoint'), {
      target: { value: '/Users/ada/.colima/work/docker.sock' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save and restart' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/system', {
      body: {
        port: 4500,
        docker_endpoint: '/Users/ada/.colima/work/docker.sock',
        log_level: 'info',
      },
    })
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/system/restart'),
    )
    expect(await screen.findByText('Pagis is starting again.')).toBeTruthy()
  })

  it('does not restart when the saved change needs no restart', async () => {
    const api = stubApi({
      PUT: vi.fn(async () => ({
        data: {
          settings: { ...settings, docker_endpoint: 'unix:///Users/ada/.colima/work/docker.sock' },
          restart_required: false,
        },
      })),
    })
    mount(api)

    fireEvent.change(await screen.findByLabelText('Docker endpoint'), {
      target: { value: 'unix:///Users/ada/.colima/work/docker.sock' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save and restart' }))

    expect(await screen.findByText('Saved.')).toBeTruthy()
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('clears the override back to discovery', async () => {
    const api = stubApi({
      GET: vi.fn(async () => ({
        data: { ...settings, docker_endpoint: 'unix:///Users/ada/.colima/work/docker.sock' },
      })),
    })
    mount(api)

    fireEvent.change(await screen.findByLabelText('Docker endpoint'), {
      target: { value: '' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save and restart' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    expect(
      (api.PUT as ReturnType<typeof vi.fn>).mock.calls[0][1].body.docker_endpoint,
    ).toBe(null)
  })

  it('shows the message when a save is refused', async () => {
    const api = stubApi({
      PUT: vi.fn(async () => ({
        error: { error: { code: 'validation', message: 'Docker did not answer at that endpoint' } },
      })),
    })
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Save and restart' }))

    expect(await screen.findByText('Docker did not answer at that endpoint')).toBeTruthy()
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('probes again on the button', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Probe again' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/system/docker/probe'),
    )
  })

  it('restarts the daemon without a save', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Restart now' }))

    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/system/restart'))
    expect(api.PUT).not.toHaveBeenCalled()
    expect(await screen.findByText('Pagis is starting again.')).toBeTruthy()
  })

  it('says to run pagis again when nothing supervises the daemon', async () => {
    const api = stubApi({ GET: vi.fn(async () => ({ data: { ...settings, supervised: false } })) })
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Restart now' }))

    expect(await screen.findByText(/Nothing starts it again here: run pagis/)).toBeTruthy()
  })

  it('says Pagis runs again once a new process answers', async () => {
    let restarted = false
    const api = stubApi({
      GET: vi.fn(async () => ({
        data: restarted ? { ...settings, started_at: 2 } : settings,
      })),
      POST: vi.fn(async (path: string) => {
        if (path === '/api/v1/system/restart') restarted = true
        return { data: { exit_code: 75 } }
      }),
    })
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Restart now' }))

    expect(await screen.findByText('Pagis is running again.', {}, { timeout: 3000 })).toBeTruthy()
  })

  describe('with a clock', () => {
    afterEach(() => {
      vi.useRealTimers()
    })

    it('says the daemon did not come back after the timeout', async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true })
      // The daemon stops at the restart and never answers again.
      let stopped = false
      const api = stubApi({
        GET: vi.fn(async () => {
          if (stopped) throw new TypeError('Failed to fetch')
          return { data: settings }
        }),
        POST: vi.fn(async () => {
          stopped = true
          return { data: { exit_code: 75 } }
        }),
      })
      mount(api)

      fireEvent.click(await screen.findByRole('button', { name: 'Restart now' }))
      await screen.findByText('Pagis is starting again.')
      await vi.advanceTimersByTimeAsync(61_000)

      expect(await screen.findByText(/did not come back/)).toBeTruthy()
    })
  })

  it('says when a flag or a variable overrides the port of the file', async () => {
    const api = stubApi({
      GET: vi.fn(async () => ({
        data: { ...settings, listening_port: 4500, port_override: 'PAGIS_PORT' },
      })),
    })
    mount(api)

    expect(
      await screen.findByText(/Pagis listens on 4500 for this run/),
    ).toBeTruthy()
    expect(
      screen.getByText(/the PAGIS_PORT variable overrides the 4400 in config.toml/),
    ).toBeTruthy()
  })

  /** A saved port differs from the one the daemon listens on until the
   *  restart puts it in effect. Nothing overrides it then. */
  it('says nothing overrides a saved port that the next start takes', async () => {
    const api = stubApi({
      GET: vi.fn(async () => ({
        data: { ...settings, port: 4410, listening_port: 4400, port_override: null },
      })),
    })
    mount(api)

    await screen.findByText('About')
    expect(screen.queryByText(/for this run/)).toBeNull()
    expect(screen.getByText('the Client App reads it before it attaches')).toBeTruthy()
  })
})
