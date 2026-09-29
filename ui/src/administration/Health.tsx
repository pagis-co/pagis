// The health of the daemon: what it runs, what it keeps its
// records in, where it reaches Docker, and how much work waits.

import type { ApiClient } from '../api/client'
import { Badge, Frame, Row } from '../primitives'
import { useInstallationHealth } from '../queries'

const DATABASE_NAMES: Record<string, string> = {
  sqlite: 'SQLite, in the data directory',
  postgres: 'Postgres',
}

export function Health({ api }: { api: ApiClient }) {
  const health = useInstallationHealth(api)

  if (!health.data) {
    return (
      <section className="administration-section">
        <div className="administration-title">
          <h2>Health</h2>
        </div>
        <p className="administration-note">
          {health.isError ? 'The health could not be read.' : 'Reading…'}
        </p>
      </section>
    )
  }
  const data = health.data
  return (
    <section className="administration-section">
      <div className="administration-title">
        <h2>Health</h2>
        <span>The daemon behind both ports.</span>
      </div>
      <Frame>
        <Row>
          <span className="administration-key">Version</span>
          <span className="administration-mono">pagis {data.version}</span>
        </Row>
        <Row>
          <span className="administration-key">Database</span>
          <span className="administration-mono">
            {DATABASE_NAMES[data.database] ?? data.database}
          </span>
        </Row>
        <Row>
          <span className="administration-key">Docker</span>
          {data.docker_endpoint === null || data.docker_endpoint === undefined ? (
            <>
              <Badge tone="failed">Unreachable</Badge>
              <span className="administration-note">
                Pagis found no Docker, so sprite computers cannot run.
              </span>
            </>
          ) : (
            <>
              <Badge tone="working">Reachable</Badge>
              <span className="administration-mono">{data.docker_endpoint}</span>
            </>
          )}
        </Row>
        <Row>
          <span className="administration-key">Volume quota</span>
          {data.volume_quota === 'supported' ? (
            <Badge tone="working">Supported</Badge>
          ) : data.volume_quota === 'unsupported' ? (
            <>
              <Badge tone="failed">Unsupported</Badge>
              <span className="administration-note">
                This machine holds no computer volume to its size, so nothing
                bounds what a sprite keeps in its volume. Put the Docker data
                directory on XFS with project quotas.
              </span>
            </>
          ) : (
            <>
              <Badge tone="waiting">Unknown</Badge>
              <span className="administration-note">
                Docker did not answer, or the computer limits set no volume
                size.
              </span>
            </>
          )}
        </Row>
        <Row>
          <span className="administration-key">Container quota</span>
          {data.container_quota === 'supported' ? (
            <Badge tone="working">Supported</Badge>
          ) : data.container_quota === 'unsupported' ? (
            <>
              <Badge tone="failed">Unsupported</Badge>
              <span className="administration-note">
                This machine holds no computer's container layer to its size, so
                a sprite can fill the Docker disk outside its volume while its
                computer is awake. Use the overlay2 storage driver, with the
                Docker data directory on XFS with project quotas.
              </span>
            </>
          ) : (
            <>
              <Badge tone="waiting">Unknown</Badge>
              <span className="administration-note">
                No computer has woken since the daemon started, or the computer
                limits set no layer size.
              </span>
            </>
          )}
        </Row>
        <Row>
          <span className="administration-key">Queue</span>
          <span className="administration-mono">
            {data.queued_runs} queued · {data.running_runs} running
          </span>
          <span className="administration-note">
            {data.unfinished_runs} runs have not finished, which counts the runs
            parked on a person.
          </span>
        </Row>
      </Frame>
    </section>
  )
}
