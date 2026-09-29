import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { ConnectionStatus } from './ConnectionStatus'

describe('ConnectionStatus', () => {
  it.each([
    ['connected', 'Connected', 'connection-status-connected'],
    ['connecting', 'Connecting', 'connection-status-warning'],
    ['reauth_required', 'Reconnect required', 'connection-status-warning'],
    ['disconnected', 'Not connected', 'connection-status-error'],
    ['unavailable', 'Unavailable', 'connection-status-error'],
  ])('shows %s with its shared label and tone', (status, label, className) => {
    render(<ConnectionStatus status={status} />)

    expect(screen.getByRole('status', { name: label }).className).toContain(
      className,
    )
  })
})
