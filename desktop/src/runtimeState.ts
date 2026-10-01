import * as fs from 'node:fs'
import * as path from 'node:path'
import semver from 'semver'

import { readExact, requireRoot, writeAtomic } from './clientFiles'
import { type ClientPlatform, thisPlatform } from './runtimeLock'

interface ActiveState {
  release: string
  platform: string
  arch: string
}

interface LaunchState {
  release: string
  previous_release: string | null
}

export class RuntimeState {
  constructor(
    private readonly root: string,
    private readonly target: ClientPlatform = thisPlatform,
  ) {}

  active(): ActiveState | null {
    requireRoot(this.root)
    const active = readExact<ActiveState>(path.join(this.root, 'active.json'), ['release', 'platform', 'arch'])
    if (active && (
      active.platform !== this.target.platform || active.arch !== this.target.arch || !release(active.release)
    )) {
      throw new Error('active.json names an invalid or unsupported runtime')
    }
    return active
  }

  launch(): LaunchState | null {
    requireRoot(this.root)
    const launch = readExact<LaunchState>(path.join(this.root, 'launch.json'), ['release', 'previous_release'])
    if (launch && (
      !release(launch.release) ||
      (launch.previous_release !== null && !release(launch.previous_release))
    )) {
      throw new Error('launch.json has an invalid release')
    }
    return launch
  }

  releaseToStart(): string | null {
    return this.highestRecordedRelease()
  }

  beginLaunch(release: string): void {
    // A launch marker names a release that may have opened or migrated the
    // Workspace. Neither a reinstalled client nor cleared active state may
    // lower that boundary.
    this.assertNotOlder(release)
    writeAtomic(path.join(this.root, 'launch.json'), {
      release,
      previous_release: this.active()?.release ?? null,
    })
  }

  activate(release: string): void {
    // An already-running server can accept a client without spawning it. Keep
    // the same downgrade boundary when that client records the active release.
    this.assertNotOlder(release)
    writeAtomic(path.join(this.root, 'active.json'), {
      release,
      platform: this.target.platform,
      arch: this.target.arch,
    })
    fs.rmSync(path.join(this.root, 'launch.json'), { force: true })
  }

  private highestRecordedRelease(): string | null {
    const launch = this.launch()?.release
    const active = this.active()?.release
    if (!launch) return active ?? null
    if (!active) return launch
    return semver.compare(parseRelease(launch), parseRelease(active)) >= 0 ? launch : active
  }

  private assertNotOlder(release: string): void {
    const attempted = parseRelease(release)
    const recorded = this.highestRecordedRelease()
    if (recorded && semver.compare(attempted, parseRelease(recorded)) < 0) {
      throw new Error(
        `This client cannot start Pagis ${release} because newer Pagis ${recorded} may have opened the Workspace. Install Pagis ${recorded} or newer.`,
      )
    }
  }
}

/**
 * What a start of the Client App does with the release that its Local
 * Installation records (`releaseToStart`): it starts its own release
 * again, upgrades an older one with no setup question (ADR-0027), or
 * shows setup. Setup also comes for a newer release, which this client
 * then refuses to start.
 */
export function startAction(recorded: string | null, own: string): 'resume' | 'upgrade' | 'setup' {
  if (recorded === own) return 'resume'
  if (recorded !== null && semver.lt(parseRelease(recorded), parseRelease(own))) return 'upgrade'
  return 'setup'
}

function release(value: unknown): value is string {
  return typeof value === 'string' && semver.valid(value) !== null
}

function parseRelease(value: string): semver.SemVer {
  const parsed = semver.parse(value)
  if (!parsed) throw new Error(`${value} is not valid SemVer`)
  return parsed
}
