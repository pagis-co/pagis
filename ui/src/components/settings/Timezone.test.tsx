import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { TimezoneSection } from './Timezone'

const workspace = {
  id: 'w-1',
  name: 'Lin',
  timezone: 'Etc/UTC',
  chief_of_staff_agent_id: 'a-1',
  report_schedule_id: 's-1',
}

function stubApi() {
  return {
    GET: vi.fn(async () => ({ data: workspace })),
    PUT: vi.fn(async (_path: string, init: { body: { timezone: string } }) => ({
      data: { ...workspace, timezone: init.body.timezone },
    })),
  } as unknown as ApiClient & { PUT: ReturnType<typeof vi.fn> }
}

function mount(api: ApiClient) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <TimezoneSection api={api} />
    </QueryClientProvider>,
  )
}

describe('TimezoneSection', () => {
  it('shows the saved timezone and saves the device timezone', async () => {
    const device = Intl.DateTimeFormat().resolvedOptions().timeZone
    const api = stubApi()
    mount(api)

    const select = await screen.findByRole('combobox', { name: 'Timezone' })
    expect(select.textContent).toContain('Etc/UTC')
    const save = screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement
    expect(save.disabled).toBe(true)

    // The test runs where the device is not on Etc/UTC or is: either
    // way the Person can pick another zone and save it.
    const target = device === 'Etc/UTC' ? 'Europe/Berlin' : device
    if (device !== 'Etc/UTC') {
      fireEvent.click(screen.getByRole('button', { name: `Use this device's timezone (${device})` }))
    } else {
      fireEvent.click(select)
      fireEvent.click(await screen.findByRole('option', { name: 'Europe/Berlin' }))
    }
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/workspace/timezone', {
        body: { timezone: target },
      }),
    )
  })
})
