import { expect, it, vi } from 'vitest'
import { probeHealth } from './health'

it('reads a permanent redirect before it checks health at the new origin', async () => {
  const request = vi.fn().mockResolvedValueOnce(new Response(null, { status: 301, headers: { Location: 'https://new.example/api/v1/health' } }))
    .mockResolvedValueOnce(Response.json({ status: 'ok', version: '0.4.1' }))
  const moved = vi.fn(() => 'https://new.example/')
  expect(await probeHealth('https://old.example/', 1000, request, undefined, moved)).toEqual({ status: 'ok', version: '0.4.1' })
  expect(request.mock.calls[0][1].redirect).toBe('manual')
  expect(moved).toHaveBeenCalledWith(301, 'https://old.example/api/v1/health', 'https://new.example/api/v1/health')
  expect(String(request.mock.calls[1][0])).toBe('https://new.example/api/v1/health')
})
