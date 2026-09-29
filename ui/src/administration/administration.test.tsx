// The Administration Interface: the three states of the page, the
// views, and what each view shows from the daemon's answers.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { Administration, viewOfPath } from './Administration'
import { Health } from './Health'
import { Resources, bytes } from './Resources'
import { Sessions } from './Sessions'
import { Spend } from './Spend'

const ADMINISTRATOR = {
  id: 'u-1',
  name: 'Ada',
  email: 'ada@example.com',
  role: 'administrator',
}

function person(id: string, name: string) {
  return {
    id,
    email: `${name.toLowerCase()}@example.com`,
    name,
    role: 'member',
    workspace_id: `w-${id}`,
    disabled: false,
    last_signed_in_at: 1_700_000_000_000,
    monthly_spend_cap_usd: null,
    created_at: 1_700_000_000_000,
  }
}

function total(costUsd: number) {
  return {
    input_tokens: 1_200,
    output_tokens: 300,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    cost_usd: costUsd,
    calls: 2,
  }
}

/** A client that answers each path with the body the test names, and
 *  throws the daemon's error shape for a path it was given none for. */
function apiOf(answers: Record<string, unknown>, errors: Record<string, unknown> = {}) {
  // A set-up installation answers the first-run read with `410 Gone`,
  // which is what says it is set up. A test that wants the setup page
  // answers that read instead.
  const refused: Record<string, unknown> = {
    '/api/v1/setup': { error: { code: 'setup_complete', message: 'gone' } },
    ...errors,
  }
  return {
    GET: vi.fn(async (path: string) =>
      path in refused && !(path in answers)
        ? { error: refused[path] }
        : { data: answers[path] ?? {} },
    ),
    POST: vi.fn(async () => ({ data: {} })),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({ data: {} })),
  } as unknown as ApiClient
}

function mount(node: React.ReactElement) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={queryClient}>{node}</QueryClientProvider>)
}

describe('the administration page', () => {
  it('shows the sign-in while the browser holds no session', async () => {
    const api = apiOf({}, { '/api/v1/user': { error: { code: 'unauthorized', message: 'no' } } })

    mount(<Administration api={api} />)

    expect(await screen.findByLabelText('Sign in')).toBeTruthy()
  })

  /** A member gets nothing, not a partial view: the port answers `403`
   *  and the page says why rather than drawing an empty interface. */
  it('says so to a person who is not an administrator', async () => {
    const api = apiOf(
      {},
      { '/api/v1/user': { error: { code: 'forbidden', message: 'not yours' } } },
    )

    mount(<Administration api={api} />)

    const refusal = await screen.findByRole('alert')
    expect(refusal.textContent).toContain('member')
    expect(screen.queryByRole('navigation', { name: 'Administration' })).toBeNull()
  })

  it('opens on the spend of the installation and moves between views', async () => {
    const api = apiOf({
      '/api/v1/user': ADMINISTRATOR,
      '/api/v1/administration/usage': {
        from: 0,
        to: 1,
        items: [{ person: person('u-2', 'Grace'), total: total(2.5), cap_reached: false }],
        total: total(2.5),
      },
      '/api/v1/administration/sessions': {
        items: [
          {
            id: 's-1',
            person: person('u-2', 'Grace'),
            client_kind: 'desktop',
            created_at: 1_700_000_000_000,
            last_used_at: 1_700_000_100_000,
            expires_at: 1_800_000_000_000,
            current: false,
          },
        ],
      },
    })

    mount(<Administration api={api} />)

    expect(await screen.findByRole('heading', { name: 'Spend' })).toBeTruthy()
    // The figure is the installation's total and the one person's, so
    // it reads twice.
    await waitFor(() => expect(screen.getAllByText('$2.50')).toHaveLength(2))

    await userEvent.click(screen.getByRole('button', { name: 'Sessions' }))

    expect(await screen.findByRole('heading', { name: 'Sessions' })).toBeTruthy()
    expect(await screen.findByText('Client App')).toBeTruthy()
  })

  /** The Org's Plugins are managed here, and not in the product. */
  it('manages the Plugins of the installation', async () => {
    const api = apiOf({ '/api/v1/user': ADMINISTRATOR, '/api/v1/plugins': { items: [] } })

    mount(<Administration api={api} />)

    await userEvent.click(await screen.findByRole('button', { name: 'Plugins' }))

    expect(await screen.findByText('No plugin is installed.')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Install a plugin' })).toBeTruthy()
  })

  /** A server nobody can sign in to has the first-run setup and nothing
   *  else, and the page is gone once somebody can. */
  it('shows the first-run setup while nobody can sign in', async () => {
    const api = apiOf({
      '/api/v1/setup': { providers: ['anthropic', 'openai'], configured_providers: ['openai'] },
    })

    mount(<Administration api={api} />)

    expect(await screen.findByLabelText('Set up this installation')).toBeTruthy()
    expect(screen.getByLabelText('Anthropic key')).toBeTruthy()
    // A provider the deployment already configured is not asked for again.
    expect(screen.queryByLabelText('OpenAI key')).toBeNull()
    expect(screen.queryByRole('navigation', { name: 'Administration' })).toBeNull()
  })

  it('takes the first administrator and the installation keys', async () => {
    const api = apiOf({
      '/api/v1/setup': { providers: ['anthropic'], configured_providers: [] },
    })

    mount(<Administration api={api} />)

    await userEvent.type(await screen.findByLabelText('Email address'), 'ada@example.net')
    await userEvent.type(screen.getByLabelText('Password'), 'correct horse battery')
    await userEvent.type(screen.getByLabelText('Anthropic key'), 'sk-key')
    await userEvent.click(screen.getByRole('button', { name: 'Make this administrator' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/setup', {
        body: {
          email: 'ada@example.net',
          password: 'correct horse battery',
          name: undefined,
          provider_keys: { anthropic: 'sk-key' },
          timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
        },
      }),
    )
  })

  /** The setup answers 200, then the setup read answers 410 and the
   *  user read answers the new Administrator: the page opens the
   *  Administration Interface with no reload. */
  it('opens the interface once the setup made the administrator', async () => {
    let made = false
    const api = {
      GET: vi.fn(async (path: string) => {
        if (path === '/api/v1/setup') {
          return made
            ? { error: { error: { code: 'setup_complete', message: 'gone' } } }
            : { data: { administration_origin: 'http://127.0.0.1:4701', providers: ['anthropic'], configured_providers: [] } }
        }
        if (path === '/api/v1/user') {
          return made
            ? { data: ADMINISTRATOR }
            : { error: { error: { code: 'unauthorized', message: 'no' } } }
        }
        return { data: {} }
      }),
      POST: vi.fn(async (path: string) => {
        if (path === '/api/v1/setup') made = true
        return { data: ADMINISTRATOR }
      }),
      PUT: vi.fn(async () => ({ data: {} })),
      DELETE: vi.fn(async () => ({ data: {} })),
    } as unknown as ApiClient

    mount(<Administration api={api} />)

    await userEvent.type(await screen.findByLabelText('Email address'), 'ada@example.net')
    await userEvent.type(screen.getByLabelText('Password'), 'correct horse battery')
    await userEvent.click(screen.getByRole('button', { name: 'Make this administrator' }))

    expect(await screen.findByRole('navigation', { name: 'Administration' })).toBeTruthy()
    expect(screen.queryByLabelText('Set up this installation')).toBeNull()
  })

  it('signs the administrator out from the header', async () => {
    const api = apiOf({
      '/api/v1/user': ADMINISTRATOR,
      '/api/v1/administration/usage': {
        from: 0,
        to: 1,
        items: [],
        total: total(0),
      },
    })

    mount(<Administration api={api} />)

    await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/sessions/current'),
    )
  })

  it('opens the view the address names', () => {
    expect(viewOfPath('/resources')).toBe('resources')
    expect(viewOfPath('/')).toBe('spend')
    expect(viewOfPath('/nothing-of-the-sort')).toBe('spend')
  })
})

describe('Spend', () => {
  it('names each person, their tokens and the cap they reached', async () => {
    const api = apiOf({
      '/api/v1/administration/usage': {
        from: 0,
        to: 1,
        items: [
          { person: person('u-2', 'Grace'), total: total(9), cap_reached: true },
          { person: person('u-3', 'Alan'), total: total(1), cap_reached: false },
        ],
        total: total(10),
      },
    })

    mount(<Spend api={api} />)

    expect(await screen.findByText('Grace')).toBeTruthy()
    expect(screen.getByText('$10.00')).toBeTruthy()
    expect(screen.getByText('$9.00')).toBeTruthy()
    expect(screen.getByText('Cap reached')).toBeTruthy()
    expect(screen.getAllByText(/1,200 in · 300 out/)).toHaveLength(2)
  })

  it('says a period with no model call has none', async () => {
    const api = apiOf({
      '/api/v1/administration/usage': { from: 0, to: 1, items: [], total: total(0) },
    })

    mount(<Spend api={api} />)

    expect(await screen.findByText('Nobody spent anything in this period.')).toBeTruthy()
  })
})

describe('Sessions', () => {
  it('marks the session the administrator is reading with', async () => {
    const api = apiOf({
      '/api/v1/administration/sessions': {
        items: [
          {
            id: 's-1',
            person: person('u-1', 'Ada'),
            client_kind: 'browser',
            created_at: 1_700_000_000_000,
            last_used_at: 1_700_000_100_000,
            expires_at: 1_800_000_000_000,
            current: true,
          },
        ],
      },
    })

    mount(<Sessions api={api} />)

    expect(await screen.findByText('This session')).toBeTruthy()
    expect(screen.getByText('Browser')).toBeTruthy()
    expect(screen.getByText('1 signed in')).toBeTruthy()
  })

  it('names a Client App session by the machine it runs on', async () => {
    const api = apiOf({
      '/api/v1/administration/sessions': {
        items: [
          {
            id: 's-2',
            person: person('u-1', 'Ada'),
            client_kind: 'desktop',
            client_name: 'macbookpro.lan',
            created_at: 1_700_000_000_000,
            last_used_at: 1_700_000_100_000,
            expires_at: 1_800_000_000_000,
            current: false,
          },
        ],
      },
    })

    mount(<Sessions api={api} />)

    expect(await screen.findByText('Client App · macbookpro.lan')).toBeTruthy()
    expect(screen.queryByText('Browser')).toBeNull()
  })

  it('says when nobody is signed in', async () => {
    const api = apiOf({ '/api/v1/administration/sessions': { items: [] } })

    mount(<Sessions api={api} />)

    expect(await screen.findByText('Nobody is signed in.')).toBeTruthy()
  })

  /** A host action needs a connected client and not only a Session, so
   *  the view says which machines are here now. */
  it('names every machine of the installation and whether it is connected', async () => {
    const api = apiOf({
      '/api/v1/administration/sessions': { items: [] },
      '/api/v1/administration/hosts': {
        items: [
          {
            id: 'h-1',
            person: person('u-2', 'Grace'),
            name: 'Air',
            platform: 'macos',
            capabilities: ['shell'],
            present: true,
            last_seen_at: 1_700_000_000_000,
          },
          {
            id: 'h-2',
            person: person('u-2', 'Grace'),
            name: 'Studio',
            platform: 'linux',
            capabilities: ['shell'],
            present: false,
            last_seen_at: 1_700_000_000_000,
          },
        ],
      },
    })

    mount(<Sessions api={api} />)

    expect(await screen.findByText('Air')).toBeTruthy()
    expect(screen.getByText('Studio')).toBeTruthy()
    expect(screen.getByText('Connected')).toBeTruthy()
    expect(screen.getByText(/last seen/)).toBeTruthy()
    expect(screen.getByText('1 of 2 connected')).toBeTruthy()
  })

  it('says when no machine is registered', async () => {
    const api = apiOf({
      '/api/v1/administration/sessions': { items: [] },
      '/api/v1/administration/hosts': { items: [] },
    })

    mount(<Sessions api={api} />)

    expect(await screen.findByText('No computer is registered.')).toBeTruthy()
  })
})

describe('Resources', () => {
  it('counts the containers, the volumes and the disk of each person', async () => {
    const api = apiOf({
      '/api/v1/administration/resources': {
        items: [
          {
            person: person('u-2', 'Grace'),
            containers: 2,
            volumes: 3,
            volume_bytes: 1_610_612_736,
            awake_computers: 1,
          },
        ],
        awake_on_server: 1,
        awake_cap_per_server: 4,
        awake_cap_per_tenant: 2,
      },
    })

    mount(<Resources api={api} />)

    expect(await screen.findByText('2 containers')).toBeTruthy()
    expect(screen.getByText('3 volumes')).toBeTruthy()
    expect(screen.getByText('1.5 GB')).toBeTruthy()
    expect(screen.getByText('1 awake')).toBeTruthy()
    expect(screen.getByText('1 awake of 4')).toBeTruthy()
  })

  /** Docker that cannot answer leaves the figure out and the page
   *  standing. */
  it('says the disk is unread where Docker did not answer', async () => {
    const api = apiOf({
      '/api/v1/administration/resources': {
        items: [
          {
            person: person('u-2', 'Grace'),
            containers: 0,
            volumes: 0,
            volume_bytes: null,
            awake_computers: 0,
          },
        ],
        awake_on_server: 0,
        awake_cap_per_server: 4,
        awake_cap_per_tenant: 2,
      },
    })

    mount(<Resources api={api} />)

    expect(await screen.findByText('disk unread')).toBeTruthy()
  })

  /** An empty list and zero caps before the read answers would say
   *  that nobody has a computer and that none may wake. */
  it('shows no figure until the read answers', async () => {
    let answer: (value: unknown) => void = () => undefined
    const pending = new Promise((resolve) => {
      answer = resolve
    })
    const api = {
      ...apiOf({}),
      GET: vi.fn(async () => pending),
    } as unknown as ApiClient

    mount(<Resources api={api} />)

    expect(await screen.findByText('Reading…')).toBeTruthy()
    expect(screen.queryByText(/Nobody has a sprite computer/)).toBeNull()
    expect(screen.queryByText(/0 awake of 0/)).toBeNull()
    answer({
      data: { items: [], awake_on_server: 1, awake_cap_per_server: 24, awake_cap_per_tenant: 3 },
    })
    expect(await screen.findByText('1 awake of 24')).toBeTruthy()
    expect(screen.getByText('3 at once per person')).toBeTruthy()
  })

  it('reads bytes the way a person does', () => {
    expect(bytes(512)).toBe('512 B')
    expect(bytes(1024)).toBe('1.0 KB')
    expect(bytes(1_610_612_736)).toBe('1.5 GB')
  })
})

describe('Health', () => {
  it('names the version, the database, Docker and the queue', async () => {
    const api = apiOf({
      '/api/v1/administration/health': {
        version: '0.1.0',
        database: 'postgres',
        docker_endpoint: 'unix:///var/run/docker.sock',
        queued_runs: 2,
        running_runs: 1,
        unfinished_runs: 4,
      },
    })

    mount(<Health api={api} />)

    expect(await screen.findByText('pagis 0.1.0')).toBeTruthy()
    expect(screen.getByText('Postgres')).toBeTruthy()
    expect(screen.getByText('unix:///var/run/docker.sock')).toBeTruthy()
    expect(screen.getByText('2 queued · 1 running')).toBeTruthy()
  })

  /** The daemon asks Docker when it has no answer, so an unknown quota
   *  says why, and never that no volume exists. */
  it('says why the volume quota is unknown', async () => {
    const api = apiOf({
      '/api/v1/administration/health': {
        version: '0.1.0',
        database: 'sqlite',
        docker_endpoint: null,
        queued_runs: 0,
        running_runs: 0,
        unfinished_runs: 0,
        volume_quota: 'unknown',
      },
    })

    mount(<Health api={api} />)

    expect(await screen.findByText(/Docker did not answer/)).toBeTruthy()
    expect(screen.queryByText(/No computer volume has been made/)).toBeNull()
  })

  /** The writable layer of a computer has a bound of its own, next to
   *  the volume. An unsupported answer says what an agent can then fill
   *  and what storage gives the bound. */
  it('reports the container quota next to the volume quota', async () => {
    const api = apiOf({
      '/api/v1/administration/health': {
        version: '0.1.0',
        database: 'sqlite',
        docker_endpoint: 'unix:///var/run/docker.sock',
        queued_runs: 0,
        running_runs: 0,
        unfinished_runs: 0,
        volume_quota: 'supported',
        container_quota: 'unsupported',
      },
    })

    mount(<Health api={api} />)

    expect(await screen.findByText('Container quota')).toBeTruthy()
    expect(screen.getByText('Supported')).toBeTruthy()
    expect(screen.getByText('Unsupported')).toBeTruthy()
    expect(screen.getByText(/outside its volume/)).toBeTruthy()
    expect(screen.getByText(/overlay2 storage driver/)).toBeTruthy()
  })

  /** The daemon learns the container quota at a wake and makes no probe
   *  container, so an unknown answer says that no computer has woken. */
  it('says why the container quota is unknown', async () => {
    const api = apiOf({
      '/api/v1/administration/health': {
        version: '0.1.0',
        database: 'sqlite',
        docker_endpoint: 'unix:///var/run/docker.sock',
        queued_runs: 0,
        running_runs: 0,
        unfinished_runs: 0,
        volume_quota: 'supported',
        container_quota: 'unknown',
      },
    })

    mount(<Health api={api} />)

    expect(await screen.findByText(/No computer has woken/)).toBeTruthy()
  })

  it('says sprite computers cannot run while Docker is unreachable', async () => {
    const api = apiOf({
      '/api/v1/administration/health': {
        version: '0.1.0',
        database: 'sqlite',
        docker_endpoint: null,
        queued_runs: 0,
        running_runs: 0,
        unfinished_runs: 0,
      },
    })

    mount(<Health api={api} />)

    expect(await screen.findByText('Unreachable')).toBeTruthy()
    expect(screen.getByText(/sprite computers cannot run/)).toBeTruthy()
  })
})
