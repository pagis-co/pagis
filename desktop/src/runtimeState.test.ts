import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { RuntimeState } from './runtimeState'

const MAC = { platform: 'darwin', arch: 'arm64' }

const roots: string[] = []
afterEach(() => {
  while (roots.length > 0) fs.rmSync(roots.pop()!, { recursive: true, force: true })
})

describe('client runtime state', () => {
  it('records launch before activation and clears it only after activation', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-runtime-state-'))
    roots.push(root)
    const state = new RuntimeState(root, MAC)

    state.beginLaunch('0.2.0')
    expect(state.launch()).toEqual({ release: '0.2.0', previous_release: null })
    expect(state.active()).toBeNull()

    state.activate('0.2.0')
    expect(state.active()).toEqual({ release: '0.2.0', platform: 'darwin', arch: 'arm64' })
    expect(state.launch()).toBeNull()

    state.beginLaunch('0.3.0')
    expect(state.launch()).toEqual({ release: '0.3.0', previous_release: '0.2.0' })
    expect(state.releaseToStart()).toBe('0.3.0')
  })

  it('refuses an invalid launch marker instead of falling back to active', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-runtime-state-'))
    roots.push(root)
    fs.writeFileSync(path.join(root, 'active.json'), '{"release":"0.1.0","platform":"darwin","arch":"arm64"}\n')
    fs.writeFileSync(path.join(root, 'launch.json'), '{"release":7,"previous_release":null}\n')

    expect(() => new RuntimeState(root, MAC).releaseToStart()).toThrow(/launch\.json.*release/)
    expect(() => new RuntimeState(root, MAC).beginLaunch('0.2.0')).toThrow(/launch\.json.*release/)
    expect(fs.readFileSync(path.join(root, 'launch.json'), 'utf8')).toBe('{"release":7,"previous_release":null}\n')
  })

  it('preserves the highest attempted release when an older client is reinstalled', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-runtime-state-'))
    roots.push(root)
    const state = new RuntimeState(root, MAC)
    state.activate('2.0.0-beta.2+client.4')
    state.beginLaunch('2.0.0+client.5')
    const marker = fs.readFileSync(path.join(root, 'launch.json'), 'utf8')

    expect(() => state.beginLaunch('2.0.0-beta.11+older-client')).toThrow(/newer Pagis 2\.0\.0/)
    expect(fs.readFileSync(path.join(root, 'launch.json'), 'utf8')).toBe(marker)
    expect(state.releaseToStart()).toBe('2.0.0+client.5')

    fs.writeFileSync(path.join(root, 'active.json'), '{"release":"3.0.0","platform":"darwin","arch":"arm64"}\n')
    fs.writeFileSync(path.join(root, 'launch.json'), '{"release":"2.5.0","previous_release":"2.0.0"}\n')
    expect(state.releaseToStart()).toBe('3.0.0')
    expect(() => state.beginLaunch('2.6.0')).toThrow(/newer Pagis 3\.0\.0/)
  })

  it('does not activate an older release after attaching to a newer runtime', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-runtime-state-'))
    roots.push(root)
    const state = new RuntimeState(root, MAC)
    state.activate('3.0.0')
    state.beginLaunch('4.0.0')
    const active = fs.readFileSync(path.join(root, 'active.json'), 'utf8')
    const launch = fs.readFileSync(path.join(root, 'launch.json'), 'utf8')

    expect(() => state.activate('2.0.0')).toThrow(/newer Pagis 4\.0\.0/)
    expect(fs.readFileSync(path.join(root, 'active.json'), 'utf8')).toBe(active)
    expect(fs.readFileSync(path.join(root, 'launch.json'), 'utf8')).toBe(launch)
  })

  it('uses SemVer precedence and accepts valid build metadata', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-runtime-state-'))
    roots.push(root)
    const state = new RuntimeState(root, MAC)

    state.beginLaunch('1.0.0-rc.10+signed.7')
    expect(() => state.beginLaunch('1.0.0-rc.2+signed.9')).toThrow(/newer Pagis/)
    state.beginLaunch('1.0.0+signed.1')
    state.beginLaunch('1.0.0+signed.2')
    expect(state.releaseToStart()).toBe('1.0.0+signed.2')

    expect(() => state.beginLaunch('1.0.0-01')).toThrow(/valid SemVer/)
  })

  it('records the platform of the client and refuses the active release of another', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-runtime-state-'))
    roots.push(root)

    new RuntimeState(root, { platform: 'linux', arch: 'x64' }).activate('0.2.0')

    expect(new RuntimeState(root, { platform: 'linux', arch: 'x64' }).active())
      .toEqual({ release: '0.2.0', platform: 'linux', arch: 'x64' })
    expect(() => new RuntimeState(root, { platform: 'linux', arch: 'arm64' }).active()).toThrow(/unsupported runtime/)
    expect(() => new RuntimeState(root, MAC).active()).toThrow(/unsupported runtime/)
  })
})
