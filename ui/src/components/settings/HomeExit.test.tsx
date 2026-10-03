// The Home Exit card (ADR-0029): the Person chooses one of their own
// Hosts that declared `exit`, knowing the five costs, and turns it off
// again. A Local Installation shows no card, and a card whose
// Administrator turned the Home Exit off says so.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, HomeExitDto, HomeExitHostDto } from '../../api/client'
import { HomeExit } from './HomeExit'

function host(overrides: Partial<HomeExitHostDto> = {}): HomeExitHostDto {
  return { id: 'h-1', name: 'Air', platform: 'macos', present: true, ...overrides }
}

function homeExit(overrides: Partial<HomeExitDto> = {}): HomeExitDto {
  return {
    available: true,
    administrator_turned_off: false,
    chosen: null,
    hosts: [host()],
    ...overrides,
  }
}

/** An API whose read answers `read`, and whose change answers with the
 *  chosen Host and the Computers in `notSwitched`. */
function stubApi(read: HomeExitDto, notSwitched: unknown[] = []) {
  return {
    GET: vi.fn(async () => ({ data: read })),
    PUT: vi.fn(async (_path: string, init: { body: { host_id: string } }) => ({
      data: {
        home_exit: {
          ...read,
          chosen: read.hosts.find((candidate) => candidate.id === init.body.host_id) ?? null,
        },
        not_switched: notSwitched,
      },
    })),
    DELETE: vi.fn(async () => ({
      data: { home_exit: { ...read, chosen: null }, not_switched: [] },
    })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <HomeExit api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

/** The five costs of ADR-0029, one pattern each. */
const COSTS = [
  /Sites see the internet address of Air for every page the sprites open/,
  /A block or an abuse report lands on that address, and sites can link the sprites.* accounts to the household.* own accounts/,
  /Every byte that a sprite.*s computer loads crosses that connection twice/,
  /Some internet providers forbid a proxy service in their terms/,
  /The sprites reach nothing on Air or on its local network/,
]

describe('HomeExit', () => {
  it('shows nothing on a Local Installation', async () => {
    const api = stubApi(homeExit({ available: false, hosts: [] }))
    const { container } = mount(api)

    await waitFor(() => expect(api.GET).toHaveBeenCalled())
    await waitFor(() => expect(container.textContent).toBe(''))
  })

  it('states the five costs before the Person turns it on', async () => {
    mount(stubApi(homeExit()))

    expect(await screen.findByRole('button', { name: 'Turn on' })).toBeTruthy()
    for (const cost of COSTS) expect(screen.getByText(cost)).toBeTruthy()
    expect(screen.getByText(/Sites see a data-center address/)).toBeTruthy()
  })

  it('turns on the Host that the Person chooses', async () => {
    const user = userEvent.setup()
    const api = stubApi(
      homeExit({ hosts: [host(), host({ id: 'h-2', name: 'Studio', present: false })] }),
    )
    mount(api)

    await user.click(await screen.findByRole('combobox', { name: 'Home Exit' }))
    await user.click(await screen.findByRole('option', { name: 'Studio (not connected)' }))
    expect(screen.getByText(/Sites see the internet address of Studio/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Turn on' }))

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/settings/home-exit', {
        body: { host_id: 'h-2' },
      }),
    )
    expect(
      await screen.findByText(/Your sprites.* computers reach the internet through Studio/),
    ).toBeTruthy()
  })

  it('says which Host carries the connections, and turns it off', async () => {
    const api = stubApi(homeExit({ chosen: host() }))
    mount(api)

    expect(
      await screen.findByText(/Your sprites.* computers reach the internet through Air\./),
    ).toBeTruthy()
    expect(screen.queryByText(COSTS[3])).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Turn off' }))

    await waitFor(() => expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/home-exit'))
    expect(await screen.findByRole('button', { name: 'Turn on' })).toBeTruthy()
  })

  it('says when the chosen Host is not connected', async () => {
    mount(stubApi(homeExit({ chosen: host({ present: false }), hosts: [host({ present: false })] })))

    expect(
      await screen.findByText(/Air is not connected now, so they reach the internet from the server/),
    ).toBeTruthy()
  })

  it('says that the Administrator turned it off, and keeps the choice', async () => {
    mount(stubApi(homeExit({ administrator_turned_off: true, chosen: host() })))

    expect(
      await screen.findByText(/The Administrator turned off the Home Exit for this server/),
    ).toBeTruthy()
    expect(screen.getByText(/Your choice of Air stays/)).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Turn on' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Turn off' })).toBeNull()
  })

  it('names each Computer that did not switch', async () => {
    const api = stubApi(homeExit(), [
      { agent_id: 'ag1', agent_name: 'Sage', error: 'the Exit Proxy switch was refused' },
    ])
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Turn on' }))

    expect(await screen.findByText(/Sage.*s computer did not switch/)).toBeTruthy()
    expect(screen.getByText(/the Exit Proxy switch was refused/)).toBeTruthy()
  })

  it('says how a computer becomes a choice when none can be one', async () => {
    mount(stubApi(homeExit({ hosts: [] })))

    expect(
      await screen.findByText(/Open the Pagis Client App on a computer at home and connect it to this server/),
    ).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Turn on' })).toBeNull()
  })
})
