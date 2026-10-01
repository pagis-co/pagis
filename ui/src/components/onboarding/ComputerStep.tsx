// Step three: the optional computer. Docker is detected by a
// ping of every candidate endpoint the daemon knows, so one socket
// that does not answer is never read as "no Docker" — the connection
// details list what each candidate said, and an endpoint the user
// types is pinged before it is saved.
//
// The download belongs to the daemon: the wizard asks the seeded
// sprite's computer to wake, and the daemon pulls the pinned image,
// starts the container and reports where it is. Repeated clicks join
// the one job, leaving the page does not stop it, and Settings shows
// the same progress afterwards. "Continue to Pagis" is live the whole
// time.

import { useState } from 'react'
import { CheckCircle2, ChevronDown, ChevronRight, XCircle } from 'lucide-react'

import type { ApiClient, DockerReportDto } from '../../api/client'
import { Badge, Button, Input } from '../../primitives'
import {
  errorMessage,
  useComputer,
  useRecheckDocker,
  useSetOnboardingDockerEndpoint,
  useWakeComputer,
} from '../../queries'
import { dockerSourceName } from '../dockerSource'

import { StepLayout } from './StepLayout'

/** What the daemon says one computer is doing, in the user's words. */
const STATE_LINES: Record<string, string> = {
  pulling: 'Downloading computer image',
  starting: 'Starting the computer',
  awake: 'The computer is ready',
  failed: 'The computer could not start',
}

function ConnectionDetails({
  api,
  docker,
  dockerEndpoint,
}: {
  api: ApiClient
  docker: DockerReportDto
  dockerEndpoint: string | null
}) {
  const [open, setOpen] = useState(false)
  const [endpoint, setEndpoint] = useState(dockerEndpoint ?? '')
  const save = useSetOnboardingDockerEndpoint(api)
  const Chevron = open ? ChevronDown : ChevronRight

  return (
    <div className="onboarding-details">
      <Button
        variant="link"
        size="sm"
        className="onboarding-details-toggle"
        aria-expanded={open}
        onClick={() => setOpen((was) => !was)}
      >
        Connection details
        <Chevron size={16} aria-hidden />
      </Button>
      {open ? (
        <div className="onboarding-details-body">
          <ul className="onboarding-candidates">
            {docker.candidates.length === 0 ? (
              <li>Pagis found no Docker endpoint to try on this machine.</li>
            ) : (
              docker.candidates.map((candidate) => (
                <li key={candidate.endpoint}>
                  <Badge tone={candidate.reachable ? 'working' : 'neutral'}>
                    {candidate.reachable ? 'Answers' : 'Silent'}
                  </Badge>
                  <span className="onboarding-candidate-endpoint">
                    {dockerSourceName(candidate.source)} ·{' '}
                    {candidate.endpoint}
                  </span>
                  {candidate.error === null ? null : (
                    <span className="onboarding-candidate-error">
                      {candidate.error}
                    </span>
                  )}
                </li>
              ))
            )}
          </ul>
          <label className="onboarding-field">
            <span>Docker endpoint</span>
            <Input
              placeholder="/var/run/docker.sock or tcp://host:2375"
              value={endpoint}
              onChange={(event) => setEndpoint(event.target.value)}
            />
          </label>
          {save.isError ? (
            <p className="onboarding-error" role="alert">
              {errorMessage(save.error, 'Docker did not answer at that endpoint.')}
            </p>
          ) : null}
          <div className="onboarding-details-actions">
            <Button
              disabled={save.isPending}
              onClick={() => save.mutate(endpoint.trim() === '' ? null : endpoint.trim())}
            >
              Use this endpoint
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  )
}

export function ComputerStep({
  api,
  docker,
  dockerEndpoint,
  canSetEndpoint,
  spriteId,
  onBack,
  onFinish,
  finishing,
  finishError,
}: {
  api: ApiClient
  docker: DockerReportDto
  /** The endpoint the administrator typed, or null for discovery. */
  dockerEndpoint: string | null
  /** Only an administrator sets the installation's Docker endpoint. */
  canSetEndpoint: boolean
  spriteId: string | undefined
  onBack: () => void
  onFinish: () => void
  finishing: boolean
  finishError: string | null
}) {
  const probe = useRecheckDocker()
  const wake = useWakeComputer(api, spriteId ?? '')
  const computer = useComputer(api, spriteId ?? '', { pollMs: 1000 })

  const reachable = docker.endpoint != null
  const state = computer.data?.state
  const percent = computer.data?.percent ?? 0
  const ready = state === 'awake'
  const imagePresent = computer.data?.image === 'present'

  return (
    <StepLayout
      onBack={onBack}
      action={
        <Button variant="primary" disabled={finishing} onClick={onFinish}>
          {reachable ? 'Continue to Pagis' : 'Continue without a computer'}
        </Button>
      }
    >
      <header className="onboarding-head">
        <h1>Set up a computer</h1>
        <p className="onboarding-lead">
          Let your sprites browse, work with files, and run tools.
        </p>
      </header>

      <div className="onboarding-docker">
        <p className="onboarding-docker-state">
          {reachable ? (
            <>
              <CheckCircle2
                className="onboarding-icon-ok"
                size={16}
                aria-hidden
              />
              Docker detected
              <span className="onboarding-docker-endpoint">
                {docker.endpoint}
              </span>
            </>
          ) : (
            <>
              <XCircle
                className="onboarding-icon-failed"
                size={16}
                aria-hidden
              />
              No Docker answered yet
            </>
          )}
        </p>
        {reachable ? null : (
          <p className="onboarding-hint">
            Start Docker Desktop, OrbStack, Colima, Rancher Desktop, Lima,
            Podman, or Docker Engine, then check again. Pagis tries each
            of their sockets, so one socket that stays silent does not
            mean Docker is missing.
          </p>
        )}
        {canSetEndpoint ? (
          <ConnectionDetails api={api} docker={docker} dockerEndpoint={dockerEndpoint} />
        ) : null}
        {reachable ? null : (
          <Button disabled={probe.isPending} onClick={() => probe.mutate()}>
            Check again
          </Button>
        )}
      </div>

      {reachable ? (
        <div className="onboarding-image">
          {state === undefined || state === 'off' || state === 'failed' ? (
            <>
              <p className="onboarding-hint">
                {imagePresent
                  ? 'The computer image is present and ready to start.'
                  : 'The computer image is about a gigabyte. You can use Pagis while it downloads.'}
              </p>
              {computer.data?.error ? (
                <p className="onboarding-error" role="alert">{computer.data.error}</p>
              ) : null}
              <Button
                disabled={wake.isPending || spriteId === undefined}
                onClick={() => {
                  wake.mutate()
                }}
              >
                {state === 'failed' || computer.data?.error || wake.isError
                  ? 'Try again'
                  : imagePresent ? 'Start computer' : 'Download computer image'}
              </Button>
            </>
          ) : (
            <>
              <p className="onboarding-image-state">
                {STATE_LINES[state] ?? state}
                {state === 'pulling' ? (
                  <span className="onboarding-image-percent">{percent}%</span>
                ) : null}
              </p>
              {ready ? null : (
                <div
                  className="onboarding-progress"
                  role="progressbar"
                  aria-label={state === 'pulling' ? 'Computer image download' : 'Computer startup'}
                  aria-valuenow={state === 'pulling' ? percent : undefined}
                  aria-valuemin={0}
                  aria-valuemax={100}
                >
                  <span
                    className={
                      state === 'pulling'
                        ? 'onboarding-progress-fill'
                        : 'onboarding-progress-fill onboarding-progress-indeterminate'
                    }
                    style={state === 'pulling' ? { width: `${percent}%` } : undefined}
                  />
                </div>
              )}
              <p className="onboarding-hint">
                {ready
                  ? 'Your sprites can use a computer now.'
                  : 'You can use Pagis while this finishes. Progress is in Settings under Computers.'}
              </p>
            </>
          )}
          {wake.isError ? (
            <p className="onboarding-error" role="alert">
              {errorMessage(wake.error, 'The download did not start.')}
            </p>
          ) : null}
        </div>
      ) : (
        <p className="onboarding-note">
          Without a computer your sprites still chat and use the app tools
          you configure, but they cannot browse or run commands. You can set
          a computer up later in Settings.
        </p>
      )}

      <p className="onboarding-note">
        Manage computers and connect services later in Settings.
      </p>
      {finishError ? <p className="onboarding-error" role="alert">{finishError}</p> : null}
    </StepLayout>
  )
}
