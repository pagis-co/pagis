// The installation settings (ADR-0024): the daemon's three settings in a
// two-column form, "Save and restart", Remote Access (ADR-0028), the
// Docker probe row, the anonymous analytics (ADR-0026), the Home Exit of
// a server (ADR-0029), and About.
// The installation's setup of each provider is the Providers view.
//
// They answer on the administration port alone, so the Administration
// Interface is the one surface that draws them.

import { useState } from 'react'

import type { ApiClient, DockerReportDto, SystemSettingsDto } from '../../api/client'
import { Badge, Button, Frame, Input, Row, SectionLabel, Select } from '../../primitives'
import {
  errorMessage,
  useProbeDocker,
  useSystemSettings,
  useSaveSystemSettings,
} from '../../queries'
import { dockerSourceName } from '../dockerSource'
import { Analytics } from './Analytics'
import { ModelRequestCapture } from './ModelRequestCapture'
import { HomeExitSetting } from './HomeExitSetting'
import { RemoteAccess } from './RemoteAccess'
import { restartMessage, useDaemonRestart } from './restart'

import './SystemSection.css'

const LOG_LEVELS = ['trace', 'debug', 'info', 'warn', 'error']

function DockerRow({ docker, api }: { docker: DockerReportDto; api: ApiClient }) {
  const probe = useProbeDocker(api)
  const inUse = docker.candidates.find((candidate) => candidate.endpoint === docker.endpoint)
  return (
    <Row>
      {docker.endpoint ? (
        <>
          <Badge tone="working">Reachable</Badge>
          <span className="system-docker-endpoint">
            {inUse ? `${dockerSourceName(inUse.source)} · ` : ''}
            {docker.endpoint}
          </span>
        </>
      ) : (
        <>
          <Badge tone="failed">Unreachable</Badge>
          <span>Pagis found no Docker, so sprite computers cannot run.</span>
        </>
      )}
      <Button
        className="system-row-trailing"
        disabled={probe.isPending}
        onClick={() => probe.mutate()}
      >
        Probe again
      </Button>
    </Row>
  )
}

function SystemForm({ api, settings }: { api: ApiClient; settings: SystemSettingsDto }) {
  const save = useSaveSystemSettings(api)
  const restart = useDaemonRestart(api, settings)
  const [port, setPort] = useState(String(settings.port))
  const [logLevel, setLogLevel] = useState(settings.log_level)
  const [dockerEndpoint, setDockerEndpoint] = useState(settings.docker_endpoint ?? '')

  // The button says "Save and restart", so a save that needs a restart
  // asks for one at once. The Docker endpoint takes effect without one,
  // so that save stops at "Saved.". The timezone is each Person's own,
  // in the Product App's Settings.
  const saveAndRestart = () =>
    save.mutate(
      {
        port: Number(port),
        docker_endpoint: dockerEndpoint.trim() === '' ? null : dockerEndpoint.trim(),
        log_level: logLevel,
      },
      { onSuccess: (saved) => saved.restart_required && restart.ask() },
    )

  const busy = save.isPending || restart.isPending
  const status =
    restartMessage(restart.phase, settings.supervised) ??
    (save.isSuccess && !save.data.restart_required ? 'Saved.' : null)

  return (
    <section className="system-section">
      <div className="system-title">
        <h3>System</h3>
        <span>The daemon. A change here restarts it.</span>
      </div>

      <Frame>
        <div className="system-form">
          <label>
            Port
            <Input
              type="number"
              className="system-mono"
              value={port}
              onChange={(event) => setPort(event.target.value)}
            />
            {settings.port_override && settings.listening_port !== settings.port && (
              <span className="system-row-note">
                Pagis listens on {settings.listening_port} for this run:{' '}
                {settings.port_override === '--port'
                  ? 'the --port flag'
                  : `the ${settings.port_override} variable`}{' '}
                overrides the {settings.port} in config.toml.
              </span>
            )}
          </label>
          <label>
            Log level
            <Select
              label="Log level"
              value={logLevel}
              onValueChange={setLogLevel}
              items={LOG_LEVELS.map((level) => ({ value: level, label: level }))}
            />
          </label>
          <label>
            Docker endpoint
            <Input
              className="system-mono"
              placeholder="Discovered"
              value={dockerEndpoint}
              onChange={(event) => setDockerEndpoint(event.target.value)}
            />
          </label>
        </div>
        <Row>
          <Button variant="primary" disabled={busy} onClick={saveAndRestart}>
            Save and restart
          </Button>
          <Button variant="ghost" disabled={busy} onClick={() => restart.ask()}>
            Restart now
          </Button>
          <span className="system-row-trailing system-row-note">
            {save.isError ? (
              <span role="alert" className="system-error">
                {errorMessage(save.error, 'Those settings could not be saved.')}
              </span>
            ) : status ? (
              <span role="status">{status}</span>
            ) : (
              'Unsaved changes restart the daemon on Save.'
            )}
          </span>
        </Row>
      </Frame>

      <SectionLabel>Network</SectionLabel>
      <RemoteAccess api={api} screen={settings.screen} daemon={settings} />

      <SectionLabel>Docker</SectionLabel>
      <Frame>
        <DockerRow docker={settings.docker} api={api} />
      </Frame>

      <SectionLabel>Analytics</SectionLabel>
      <Analytics api={api} analytics={settings.analytics} />

      <SectionLabel>Model requests</SectionLabel>
      <ModelRequestCapture api={api} capture={settings.model_request_capture} />

      {settings.home_exit && (
        <>
          <SectionLabel>Home Exit</SectionLabel>
          <HomeExitSetting api={api} homeExit={settings.home_exit} />
        </>
      )}

      <SectionLabel>About</SectionLabel>
      <Frame>
        <Row>
          <span className="system-about-key">Data directory</span>
          <span className="system-mono">{settings.data_directory}</span>
        </Row>
        <Row>
          <span className="system-about-key">Version</span>
          <span className="system-mono">pagis {settings.version}</span>
          <span className="system-row-note">the Client App reads it before it attaches</span>
        </Row>
      </Frame>
    </section>
  )
}

export function SystemSection({ api }: { api: ApiClient }) {
  const settings = useSystemSettings(api)
  if (!settings.data) {
    return (
      <section className="system-section">
        <div className="system-title">
          <h3>System</h3>
        </div>
        <p className="system-row-note">
          {settings.isError ? 'The system settings could not be read.' : 'Reading…'}
        </p>
      </section>
    )
  }
  // The form starts from the saved values and then holds what the user
  // types, until a save replaces them.
  return <SystemForm api={api} settings={settings.data} />
}
