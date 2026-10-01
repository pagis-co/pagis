// First run: welcome, model, computer. The daemon
// holds every durable answer, so these tests drive the wizard through
// a stubbed API and assert what it sends and what it refuses to claim.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { defaultAppearance } from '../../avatars/catalog'
import { Onboarding } from './Onboarding'

type Status = {
  completed: boolean
  providers: { provider: string; configured: boolean; source: string | null }[]
  model: { provider: string; available: number } | null
  docker: DockerReport
  docker_endpoint: string | null
}

type DockerReport = {
  endpoint: string | null
  candidates: { source: string; endpoint: string; reachable: boolean; error: string | null }[]
}

function freshStatus(): Status {
  return {
    completed: false,
    providers: [
      { provider: 'anthropic', configured: false, source: null },
      { provider: 'openai', configured: false, source: null },
      { provider: 'openrouter', configured: false, source: null },
    ],
    model: null,
    docker: dockerReport(null),
    docker_endpoint: null,
  }
}

function dockerReport(endpoint: string | null): DockerReport {
  return {
    endpoint,
    candidates: [
        {
          source: 'podman',
          endpoint: 'unix:///run/user/1000/podman/podman.sock',
          reachable: false,
          error: 'no such file or directory',
        },
        {
          source: 'system_socket',
          endpoint: 'unix:///var/run/docker.sock',
          reachable: endpoint !== null,
          error: endpoint === null ? 'permission denied' : null,
        },
      ],
  }
}

const SPRITE = {
  id: 'ag1',
  name: 'Pixie',
  job: 'general assistant',
  description: '',
  personality: '',
  status: 'active',
  avatar: defaultAppearance(),
}

interface Options {
  status?: Status
  dockerEndpoint?: string | null
  computer?: { state: string; image: string; percent: number | null; error: string | null; holder: string }
  checkFails?: string
  completeFails?: string
  role?: 'administrator' | 'member'
}

function stubApi(options: Options = {}) {
  const status = options.status ?? freshStatus()
  status.docker = dockerReport(options.dockerEndpoint ?? null)
  const computer = options.computer ?? {
    state: 'off',
    image: 'absent',
    percent: null,
    error: null,
    holder: 'agent',
  }
  const api = {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents') return { data: { items: [SPRITE] } }
      if (path === '/api/v1/user')
        return { data: { id: 'u1', name: null, email: null, role: options.role ?? 'administrator' } }
      if (path === '/api/v1/agents/{agent_id}/computer') return { data: computer }
      if (path === '/api/v1/settings/models') {
        return {
          data: {
            providers: [
              {
                provider: 'anthropic',
                error: null,
                preselected: 'claude-opus-5',
                models: ['claude-opus-5', 'claude-sonnet-4-6'].map((id) => ({
                  candidate: `anthropic/${id}`,
                  context_window: 1000000,
                  max_output_tokens: 64000,
                  input_cost: null,
                  output_cost: null,
                })),
              },
            ],
          },
        }
      }
      return { data: status }
    }),
    PUT: vi.fn(async () => {
      return {
        data: { provider: 'anthropic', configured: true, source: 'secret_file' },
        response: { ok: true },
      }
    }),
    POST: vi.fn(async (path: string) => {
      if (path === '/api/v1/settings/onboarding/providers/{provider}/key/check') {
        if (options.checkFails !== undefined) {
          return { error: { error: { message: options.checkFails } } }
        }
        status.providers[0] = { provider: 'anthropic', configured: true, source: 'secret_file' }
        status.model = { provider: 'anthropic', available: 12 }
        return { data: { provider: 'anthropic', available: 12 } }
      }
      if (path === '/api/v1/settings/providers/{provider}/check') {
        if (options.checkFails !== undefined) {
          return { error: { error: { message: options.checkFails } } }
        }
        status.model = { provider: 'anthropic', available: 12 }
        return { data: { provider: 'anthropic', available: 12 } }
      }
      if (path === '/api/v1/agents/{agent_id}/computer/wake') {
        return { data: { state: 'pulling', image: 'absent', percent: 0, error: null, holder: 'agent' } }
      }
      if (path === '/api/v1/settings/onboarding/complete' && options.completeFails) {
        return { error: { error: { message: options.completeFails } }, response: { ok: false } }
      }
      return { data: {}, response: { ok: true } }
    }),
  }
  return api
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <Onboarding api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

/** Walk from the first step to the model step. */
async function reachModel() {
  fireEvent.click(await screen.findByText('Continue'))
  await screen.findByRole('heading', { name: /Connect your model/ })
}

/** Walk from the first step to the computer step, with a key and no check. */
async function reachComputer() {
  await reachModel()
  fireEvent.change(screen.getByPlaceholderText('API key'), {
    target: { value: 'sk-test' },
  })
  fireEvent.click(screen.getByText('Continue'))
  await screen.findByRole('heading', { name: 'Set up a computer' })
}

beforeEach(() => {
  localStorage.clear()
})

/** The three parts of the step on screen: the stepper, the middle that
 *  scrolls and the footer with the actions. */
function frame() {
  const column = screen.getByRole('main', { name: 'Set up Pagis' })
  const [stepper, body, footer, ...rest] = [...column.children] as HTMLElement[]
  expect(rest).toEqual([])
  expect(stepper.className).toBe('onboarding-stepper')
  expect(body.className).toBe('onboarding-body')
  expect(footer.tagName).toBe('FOOTER')
  expect(footer.className).toBe('onboarding-actions')
  return { body, footer }
}

/** The names of the footer buttons, left to right, and the primary one. */
function actions(footer: HTMLElement) {
  const buttons = [...footer.querySelectorAll('button')]
  return {
    names: buttons.map((button) => button.textContent),
    primary: buttons
      .filter((button) => button.classList.contains('ui-button-primary'))
      .map((button) => button.textContent),
  }
}

describe('the first run page', () => {
  it('puts the title bar strip above the steps, on the ground of the window', async () => {
    const { container } = mount(stubApi())

    const steps = await screen.findByRole('main', { name: 'Set up Pagis' })
    const page = container.firstElementChild
    expect(page?.className).toBe('onboarding')
    expect(page?.firstElementChild?.className).toBe('onboarding-titlebar')
    expect(page?.firstElementChild?.getAttribute('aria-hidden')).toBe('true')
    expect(steps.className).toBe('onboarding-column')
    expect(steps.parentElement).toBe(page)
  })

  it('shows the welcome step with its one action in the footer and no Back', async () => {
    mount(stubApi())
    await screen.findByRole('heading', { name: 'Welcome to Pagis' })

    const { body, footer } = frame()
    expect(body.querySelector('h1')?.textContent).toBe('Welcome to Pagis')
    expect(body.querySelector('button')).toBeNull()
    expect(actions(footer)).toEqual({ names: ['Continue'], primary: ['Continue'] })
  })

  it('keeps the model step actions in the footer while the check shows its result', async () => {
    mount(stubApi())
    await reachModel()

    const { body, footer } = frame()
    expect(actions(footer)).toEqual({ names: ['Back', 'Continue'], primary: ['Continue'] })
    expect(body.contains(screen.getByText('Test connection'))).toBe(true)

    fireEvent.change(screen.getByPlaceholderText('API key'), {
      target: { value: 'sk-test' },
    })
    fireEvent.click(screen.getByText('Test connection'))
    const result = await screen.findByText('The key works; 12 models available')

    // The result grows the middle, and the footer stays where it is.
    const after = frame()
    expect(after.body.contains(result)).toBe(true)
    expect(after.footer).toBe(footer)
    expect(actions(after.footer)).toEqual({
      names: ['Back', 'Continue'],
      primary: ['Continue'],
    })
  })

  it('goes back from the model step to the welcome step', async () => {
    mount(stubApi())
    await reachModel()

    fireEvent.click(within(frame().footer).getByText('Back'))

    expect(await screen.findByRole('heading', { name: 'Welcome to Pagis' })).toBeTruthy()
    expect(screen.getByRole('listitem', { current: 'step' }).textContent).toContain('Welcome')
  })

  it('keeps the computer step actions in the footer and goes back to the model step', async () => {
    mount(stubApi())
    await reachComputer()

    const { body, footer } = frame()
    expect(actions(footer)).toEqual({
      names: ['Back', 'Continue without a computer'],
      primary: ['Continue without a computer'],
    })
    expect(body.contains(screen.getByText('Check again'))).toBe(true)

    fireEvent.click(within(footer).getByText('Back'))

    expect(await screen.findByRole('heading', { name: /Connect your model/ })).toBeTruthy()
    expect(screen.getByRole('listitem', { current: 'step' }).textContent).toContain('Model')
  })

  it('names the computer step action for a reachable Docker', async () => {
    mount(stubApi({ dockerEndpoint: 'unix:///var/run/docker.sock' }))
    await reachComputer()

    const { body, footer } = frame()
    expect(actions(footer)).toEqual({
      names: ['Back', 'Continue to Pagis'],
      primary: ['Continue to Pagis'],
    })
    expect(body.contains(screen.getByText('Download computer image'))).toBe(true)
  })
})

describe('the welcome step', () => {
  it('opens the first run', async () => {
    mount(stubApi())

    expect(
      await screen.findByRole('heading', { name: 'Welcome to Pagis' }),
    ).toBeTruthy()
  })

  it('explains sprites and introduces the first one', async () => {
    const api = stubApi()
    mount(api)

    expect(
      await screen.findByText(/Sprites are AI agents that work as your virtual assistants/),
    ).toBeTruthy()
    const capabilities = screen.getByRole('list', { name: '' })
    expect(
      [...capabilities.querySelectorAll('.onboarding-capability-name')].map(
        (node) => node.textContent,
      ),
    ).toEqual(['Computer', 'Memory', 'Follow-ups'])
    expect(screen.getByText('Pixie')).toBeTruthy()
    expect(screen.getByText(/Your first and main sprite/)).toBeTruthy()
    expect(screen.queryByText('Back')).toBeNull()
  })
})

describe('the model step', () => {
  it('uses the daemon proof after Back without checking the key again', async () => {
    const status = freshStatus()
    status.model = { provider: 'openrouter', available: 300 }
    const api = stubApi({ status })
    mount(api)
    await reachModel()

    expect(screen.getByText('The key works; 300 models available')).toBeTruthy()
    expect((screen.getByText('Continue') as HTMLButtonElement).disabled).toBe(false)
    expect(api.POST).not.toHaveBeenCalledWith(
      '/api/v1/settings/providers/{provider}/check',
      expect.anything(),
    )
  })

  it('continues with a typed key and does not force a check', async () => {
    const api = stubApi()
    mount(api)
    await reachModel()

    const continues = screen.getByText('Continue') as HTMLButtonElement
    expect(continues.disabled).toBe(true)

    fireEvent.change(screen.getByPlaceholderText('API key'), {
      target: { value: 'sk-test' },
    })
    expect(continues.disabled).toBe(false)
    fireEvent.click(continues)

    await screen.findByRole('heading', { name: 'Set up a computer' })
    expect(api.PUT).toHaveBeenCalledWith(
      '/api/v1/settings/onboarding/providers/{provider}/key',
      {
        params: { path: { provider: 'anthropic' } },
        body: { key: 'sk-test' },
      },
    )
    // With no list to pick from, the daemon takes its preselection.
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/onboarding/default-model', {
      body: { provider: 'anthropic', model: null },
    })
    expect(api.POST).not.toHaveBeenCalledWith(
      '/api/v1/settings/providers/{provider}/check',
      expect.anything(),
    )
  })

  it('checks the connection when asked', async () => {
    const api = stubApi()
    mount(api)
    await reachModel()

    fireEvent.change(screen.getByPlaceholderText('API key'), {
      target: { value: 'sk-test' },
    })
    fireEvent.click(screen.getByText('Test connection'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/onboarding/providers/{provider}/key/check',
        {
          params: { path: { provider: 'anthropic' } },
          body: { key: 'sk-test' },
        },
      ),
    )
    expect(
      await screen.findByText('The key works; 12 models available'),
    ).toBeTruthy()
    // The check stored the key; nothing stores it a second time.
    expect(api.PUT).not.toHaveBeenCalledWith(
      '/api/v1/settings/onboarding/providers/{provider}/key',
      expect.anything(),
    )
  })

  it('checks the held key when nothing is typed', async () => {
    const status = freshStatus()
    status.providers[0] = { provider: 'anthropic', configured: true, source: 'env' }
    const api = stubApi({ status })
    mount(api)
    await reachModel()

    fireEvent.click(screen.getByText('Test connection'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/providers/{provider}/check',
        { params: { path: { provider: 'anthropic' } } },
      ),
    )
    expect(
      await screen.findByText('The key works; 12 models available'),
    ).toBeTruthy()
  })

  it('offers the listed models, newest preselected, and saves the pick as the default route', async () => {
    const api = stubApi()
    mount(api)
    await reachModel()

    fireEvent.change(screen.getByPlaceholderText('API key'), {
      target: { value: 'sk-test' },
    })
    fireEvent.click(screen.getByText('Test connection'))

    const picker = await screen.findByRole('combobox', { name: 'Model' })
    expect(picker.textContent).toContain('claude-opus-5')
    fireEvent.click(screen.getByText('Continue'))

    await screen.findByRole('heading', { name: 'Set up a computer' })
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/onboarding/default-model', {
      body: { provider: 'anthropic', model: 'claude-opus-5' },
    })
  })

  it('a failed check shows the provider words and never reads as ready', async () => {
    const api = stubApi({ checkFails: '401 Unauthorized: invalid x-api-key' })
    mount(api)
    await reachModel()

    fireEvent.change(screen.getByPlaceholderText('API key'), {
      target: { value: 'sk-bad' },
    })
    fireEvent.click(screen.getByText('Test connection'))

    expect(
      await screen.findByText('401 Unauthorized: invalid x-api-key'),
    ).toBeTruthy()
    expect(screen.queryByText(/The key works/)).toBeNull()
    // The refused key is not stored, so Pagis holds no key and the
    // step cannot go on with the key gone from the field.
    expect(api.PUT).not.toHaveBeenCalled()
    expect(screen.queryByText(/already holds a key/)).toBeNull()
    expect((screen.getByPlaceholderText('API key') as HTMLInputElement).value).toBe('sk-bad')
    // Continue does not store the key the provider refused, and a new
    // key opens it again.
    const continues = screen.getByText('Continue') as HTMLButtonElement
    expect(continues.disabled).toBe(true)
    fireEvent.change(screen.getByPlaceholderText('API key'), {
      target: { value: 'sk-other' },
    })
    expect(continues.disabled).toBe(false)
  })

  it('names the step and Google in words a new person knows', async () => {
    mount(stubApi())
    await reachModel()

    const heading = screen.getByRole('heading', { name: /Connect your model/ })
    expect(heading.textContent).toBe('Connect your model')
    expect(screen.getByText(/Connect your Google account later/)).toBeTruthy()
    expect(screen.queryByText(/\bgog\b/)).toBeNull()
  })

  it('keeps the key masked and out of browser storage', async () => {
    const api = stubApi()
    mount(api)
    await reachModel()

    const field = screen.getByPlaceholderText('API key') as HTMLInputElement
    expect(field.type).toBe('password')
    fireEvent.change(field, { target: { value: 'sk-secret' } })

    expect(localStorage.length).toBe(0)
    expect(sessionStorage.length).toBe(0)
  })
})

describe('the computer step', () => {
  it('starts the download, shows real progress and lets the user go on', async () => {
    const api = stubApi({
      dockerEndpoint: 'unix:///var/run/docker.sock',
      computer: { state: 'pulling', image: 'absent', percent: 42, error: null, holder: 'agent' },
    })
    mount(api)
    await reachComputer()

    expect(screen.getByText('Docker detected')).toBeTruthy()
    fireEvent.click(screen.getByText('Download computer image'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/computer/wake',
        { params: { path: { agent_id: 'ag1' } } },
      ),
    )
    expect(await screen.findByText('Downloading computer image')).toBeTruthy()
    expect(screen.getByText('42%')).toBeTruthy()
    expect(screen.getByRole('progressbar').getAttribute('aria-valuenow')).toBe('42')

    // The download belongs to the daemon, so Pagis opens while it runs.
    fireEvent.click(screen.getByText('Continue to Pagis'))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/onboarding/complete',
        { body: { user_name: null } },
      ),
    )
  })

  it('names every endpoint it tried, so one silent socket is not "no Docker"', async () => {
    const api = stubApi()
    mount(api)
    await reachComputer()

    expect(screen.getByText('No Docker answered yet')).toBeTruthy()
    fireEvent.click(screen.getByText('Connection details'))

    const candidates = [
      ...document.querySelectorAll('.onboarding-candidate-endpoint'),
    ].map((node) => node.textContent)
    expect(candidates).toHaveLength(2)
    expect(candidates[0]).toBe('Podman · unix:///run/user/1000/podman/podman.sock')
    expect(candidates[1]).toBe('System socket · unix:///var/run/docker.sock')
    expect(screen.getByText('no such file or directory')).toBeTruthy()
    expect(screen.getByText('permission denied')).toBeTruthy()
  })

  it('finishes without a computer and says what sprites cannot do yet', async () => {
    const api = stubApi()
    mount(api)
    await reachComputer()

    expect(
      screen.getByText(/they cannot browse or run commands/),
    ).toBeTruthy()
    fireEvent.click(screen.getByText('Continue without a computer'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/onboarding/complete',
        { body: { user_name: null } },
      ),
    )
  })

  it('re-probes Docker when the user asks', async () => {
    const api = stubApi()
    mount(api)
    await reachComputer()

    const reads = () =>
      api.GET.mock.calls.filter((call: unknown[]) => call[0] === '/api/v1/settings/onboarding')
        .length
    const before = reads()
    fireEvent.click(screen.getAllByText('Check again')[0])

    // The onboarding read pings every candidate, so the check reads it
    // again.
    await waitFor(() => expect(reads()).toBeGreaterThan(before))
  })

  it('saves the endpoint the administrator typed through the first-run route', async () => {
    const api = stubApi()
    mount(api)
    await reachComputer()

    fireEvent.click(screen.getByText('Connection details'))
    fireEvent.change(screen.getByPlaceholderText('/var/run/docker.sock or tcp://host:2375'), {
      target: { value: ' tcp://10.0.0.2:2375 ' },
    })
    fireEvent.click(screen.getByText('Use this endpoint'))

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/onboarding/docker-endpoint', {
        body: { docker_endpoint: 'tcp://10.0.0.2:2375' },
      }),
    )
  })

  it('offers one check again with the connection details open', async () => {
    mount(stubApi())
    await reachComputer()

    fireEvent.click(screen.getByText('Connection details'))

    expect(screen.getAllByText('Check again')).toHaveLength(1)
  })

  it('offers a member no endpoint to set', async () => {
    const api = stubApi({ role: 'member' })
    mount(api)
    await reachComputer()

    expect(screen.getByText('No Docker answered yet')).toBeTruthy()
    expect(screen.queryByText('Connection details')).toBeNull()
  })

  it('shows a completion failure and keeps setup open', async () => {
    const api = stubApi({ completeFails: 'the verified model route changed' })
    mount(api)
    await reachComputer()

    fireEvent.click(screen.getByText('Continue without a computer'))

    expect(await screen.findByText('the verified model route changed')).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Set up a computer' })).toBeTruthy()
  })

  it('shows a present image separately and no progress bar once ready', async () => {
    const api = stubApi({
      dockerEndpoint: 'unix:///var/run/docker.sock',
      computer: { state: 'awake', image: 'present', percent: null, error: null, holder: 'agent' },
    })
    mount(api)
    await reachComputer()

    expect(await screen.findByText('The computer is ready')).toBeTruthy()
    expect(screen.queryByRole('progressbar')).toBeNull()
  })

  it('shows an image inspection error without blocking chat-only setup', async () => {
    const api = stubApi({
      dockerEndpoint: 'unix:///var/run/docker.sock',
      computer: {
        state: 'off',
        image: 'mismatched',
        percent: null,
        error: 'The local image does not match this Pagis release.',
        holder: 'agent',
      },
    })
    mount(api)
    await reachComputer()

    expect((await screen.findByRole('alert')).textContent).toContain(
      'The local image does not match this Pagis release.',
    )
    expect((screen.getByText('Continue to Pagis') as HTMLButtonElement).disabled).toBe(false)
  })
})
