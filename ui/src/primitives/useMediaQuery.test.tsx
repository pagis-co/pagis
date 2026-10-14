// Another app branches its layout at the same phone breakpoint as the
// shell, so the primitives entry exports the query and the hook.

import { renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { MOBILE_QUERY, useIsMobile } from '.'

describe('the phone breakpoint of the primitives entry', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('matches the phone query', () => {
    const matchMedia = vi.fn((media: string) => ({
      matches: media === MOBILE_QUERY,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }))
    vi.stubGlobal('matchMedia', matchMedia)
    expect(MOBILE_QUERY).toBe('(max-width: 760px)')
    expect(renderHook(() => useIsMobile()).result.current).toBe(true)
    expect(matchMedia).toHaveBeenCalledWith(MOBILE_QUERY)
  })
})
