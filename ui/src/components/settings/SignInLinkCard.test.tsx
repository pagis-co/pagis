// A Sign-In Link as a person hands it on: the QR code, the address with
// its copy button, and the time the link has left.

import { act, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { SignInLinkCard, timeLeft } from './SignInLinkCard'

const NOW = 1_700_000_000_000
const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

const LINK = {
  url: 'https://pagis.example/sign-in#secret',
  expires_at: NOW + 5 * MINUTE,
  qr_svg: '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
}

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('timeLeft', () => {
  it('rounds an invite to days and counts the last minutes to the second', () => {
    expect(timeLeft(NOW + 7 * DAY - 1, NOW)).toBe('Expires in 7 days.')
    expect(timeLeft(NOW + DAY, NOW)).toBe('Expires in 1 day.')
    expect(timeLeft(NOW + 3 * HOUR, NOW)).toBe('Expires in 3 hours.')
    expect(timeLeft(NOW + HOUR, NOW)).toBe('Expires in 1 hour.')
    expect(timeLeft(NOW + 4 * MINUTE + 5_000, NOW)).toBe('Expires in 4:05.')
  })

  it('says a link at or past its expiry is expired', () => {
    expect(timeLeft(NOW, NOW)).toBe('This link expired. Make a new one.')
    expect(timeLeft(NOW - 1, NOW)).toBe('This link expired. Make a new one.')
  })
})

describe('SignInLinkCard', () => {
  it('shows the QR code and the address, and copies the address', async () => {
    const writeText = vi.fn(async () => undefined)
    vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } })
    render(<SignInLinkCard link={LINK} now={() => NOW} />)

    const qr = screen.getByAltText('QR code of the sign-in link') as HTMLImageElement
    expect(decodeURIComponent(qr.src)).toContain(LINK.qr_svg)
    expect((screen.getByLabelText('Sign-in link') as HTMLInputElement).value).toBe(LINK.url)
    expect(screen.getByRole('status').textContent).toBe('Expires in 5:00. It works once.')

    await userEvent.click(screen.getByRole('button', { name: 'Copy' }))

    expect(writeText).toHaveBeenCalledWith(LINK.url)
    expect(await screen.findByRole('button', { name: 'Copied' })).toBeTruthy()
  })

  it('takes the code away and offers no copy once the link expired', () => {
    vi.useFakeTimers()
    let now = NOW + 5 * MINUTE - 1_000
    render(<SignInLinkCard link={LINK} now={() => now} />)
    expect(screen.getByAltText('QR code of the sign-in link')).toBeTruthy()

    now = NOW + 5 * MINUTE
    act(() => {
      vi.advanceTimersByTime(1_000)
    })

    expect(screen.queryByAltText('QR code of the sign-in link')).toBeNull()
    expect(screen.getByRole('button', { name: 'Copy' }).getAttribute('disabled')).not.toBeNull()
    expect(screen.getByRole('status').textContent).toContain('This link expired.')
  })
})
