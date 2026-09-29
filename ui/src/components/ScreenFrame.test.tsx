// The live screen frame: whose computer it is, who holds the
// switch, and the handback countdown inside the frame.

import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { ScreenFrame, holderOf } from './ScreenFrame'

describe('ScreenFrame', () => {
  it('names the computer and its agent', () => {
    render(
      <ScreenFrame agentId="ag1" agentName="Sage" holder="agent">
        <video aria-label="live" />
      </ScreenFrame>,
    )

    expect(screen.getByText("Sage's computer")).toBeTruthy()
    expect(screen.getByText('Sage is in control')).toBeTruthy()
    expect(screen.getByLabelText('live')).toBeTruthy()
  })

  it('says who is in control during a takeover', () => {
    render(
      <ScreenFrame agentId="ag1" agentName="Sage" holder="user">
        <video aria-label="live" />
      </ScreenFrame>,
    )

    expect(screen.getByText('You are in control')).toBeTruthy()
    expect(screen.getByTestId('screen-frame').dataset.holder).toBe('user')
  })

  it('says when the daemon holds the switch', () => {
    render(
      <ScreenFrame agentId="ag1" agentName="Sage" holder="daemon">
        <video aria-label="live" />
      </ScreenFrame>,
    )

    expect(screen.getByText('Pagis is filling a saved login')).toBeTruthy()
  })

  it('holds the countdown inside the frame', () => {
    render(
      <ScreenFrame
        agentId="ag1"
        agentName="Sage"
        holder="user"
        countdown={<span data-testid="countdown">5 s</span>}
      >
        <video aria-label="live" />
      </ScreenFrame>,
    )

    const frame = screen.getByTestId('screen-frame')
    expect(frame.contains(screen.getByTestId('countdown'))).toBe(true)
  })

  it('reads an unknown holder as the agent', () => {
    expect(holderOf('user')).toBe('user')
    expect(holderOf('daemon')).toBe('daemon')
    expect(holderOf('agent')).toBe('agent')
    expect(holderOf(undefined)).toBe('agent')
    expect(holderOf('something else')).toBe('agent')
  })
})
