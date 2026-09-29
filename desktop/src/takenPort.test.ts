import { describe, expect, it } from 'vitest'

import { parseTakenAdministrationPort, parseTakenPort, portHolder, type Probe } from './takenPort'

describe('the taken-port message', () => {
  it('reads the port from the daemon message', () => {
    expect(parseTakenPort(
      'Error: port 4400 is already in use. Stop the process that holds it, or start Pagis on ' +
        'another port with `pagis --port <PORT>`.',
    )).toBe(4400)
  })

  it('is absent from any other output, the administration port message included', () => {
    expect(parseTakenPort('pagis booted\nlistening on 127.0.0.1:4400')).toBeNull()
    expect(parseTakenPort(
      'the administration port 4401 is already in use. Stop the process that holds it, or name ' +
        'another port in `[administration] port` of config.toml.',
    )).toBeNull()
  })
})

describe('the taken Administration Port message', () => {
  it('reads the port from the daemon message', () => {
    expect(parseTakenAdministrationPort(
      'Error: the administration port 4401 is already in use. Stop the process that holds it, or ' +
        'name another port in `[administration] port` of config.toml.',
    )).toBe(4401)
  })

  it('is absent from any other output, the product port message included', () => {
    expect(parseTakenAdministrationPort('pagis booted\nlistening on 127.0.0.1:4401')).toBeNull()
    expect(parseTakenAdministrationPort(
      'Error: port 4400 is already in use. Stop the process that holds it, or start Pagis on ' +
        'another port with `pagis --port <PORT>`.',
    )).toBeNull()
  })
})

function answers(output: string | null, calls: string[][] = []): Probe {
  return async (program, args) => {
    calls.push([program, ...args])
    return output
  }
}

describe('the process that holds a port', () => {
  it('asks ss on Linux for the listener of the port', async () => {
    const calls: string[][] = []
    const holder = await portHolder(4400, 'linux', answers(
      'LISTEN 0 4096 127.0.0.1:4400 0.0.0.0:* users:(("python3",pid=4242,fd=3))\n',
      calls,
    ))

    expect(holder).toBe('python3 (pid 4242)')
    expect(calls).toEqual([['ss', '-Hltnp', 'sport = :4400']])
  })

  it('names nothing on Linux when ss is absent or shows no process of this account', async () => {
    expect(await portHolder(4400, 'linux', answers(null))).toBeNull()
    expect(await portHolder(4400, 'linux', answers('LISTEN 0 4096 0.0.0.0:4400 0.0.0.0:*\n'))).toBeNull()
  })

  it('asks lsof on macOS', async () => {
    const calls: string[][] = []
    const holder = await portHolder(4400, 'darwin', answers('p4242\nccom.docker.backend\n', calls))

    expect(holder).toBe('com.docker.backend (pid 4242)')
    expect(calls[0][0]).toBe('/usr/sbin/lsof')
    expect(await portHolder(4400, 'darwin', answers(null))).toBeNull()
  })
})
