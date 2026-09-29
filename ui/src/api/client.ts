// The typed REST client: openapi-fetch over the generated schema. The
// session is an HTTP-only cookie, which the browser sends with
// every same-origin request, so no request carries a credential of its
// own. The schema is generated from the daemon's OpenAPI spec
// (`npm run api:generate`); CI fails on drift. Artifact bytes go
// through plain fetch: multipart up, Blob down, always via the daemon.

import createClient from 'openapi-fetch'

import type { components, paths } from './schema'

export type ChannelDto = components['schemas']['ChannelDto']
export type MessageDto = components['schemas']['MessageDto']
export type ThreadDto = components['schemas']['ThreadDto']
export type TimelineItem = components['schemas']['TimelineItem']
export type PointerDto = components['schemas']['PointerDto']
export type ReplyAuthorDto = components['schemas']['ReplyAuthorDto']
export type EventRow = components['schemas']['EventRow']
export type DeltaFrame = components['schemas']['DeltaFrame']
export type ProgressFrame = components['schemas']['ProgressFrame']
export type ArtifactDto = components['schemas']['ArtifactDto']
export type GrantDto = components['schemas']['GrantDto']
export type MemoryFeedItem = components['schemas']['MemoryFeedItem']
export type AgentDto = components['schemas']['AgentDto']
export type ComputerDto = components['schemas']['ComputerDto']
export type RequestDto = components['schemas']['RequestDto']
export type KeypadCodeDto = components['schemas']['KeypadCodeDto']
export type RunDto = components['schemas']['RunDto']
export type RunTranscriptDto = components['schemas']['RunTranscriptDto']
export type RunEventDto = components['schemas']['RunEventDto']
export type RunStepsDto = components['schemas']['RunStepsDto']
export type RunStepDto = components['schemas']['RunStepDto']
export type ModelAliasDto = components['schemas']['ModelAliasDto']
export type RetentionPolicyDto = components['schemas']['RetentionPolicyDto']
export type PersonDto = components['schemas']['PersonDto']
export type PersonUsageDto = components['schemas']['PersonUsageDto']
export type InstallationUsageDto = components['schemas']['InstallationUsageDto']
export type MyUsageDto = components['schemas']['MyUsageDto']
export type SessionDto = components['schemas']['SessionDto']
export type HostDto = components['schemas']['HostDto']
export type AdministrationHostDto =
  components['schemas']['AdministrationHostDto']
export type PersonResourcesDto = components['schemas']['PersonResourcesDto']
export type ResourcesDto = components['schemas']['ResourcesDto']
export type InstallationHealthDto =
  components['schemas']['InstallationHealthDto']
export type RunSpendDto = components['schemas']['RunSpendDto']
export type UsageTotalDto = components['schemas']['UsageTotalDto']
export type SystemSettingsDto = components['schemas']['SystemSettingsDto']
export type SystemSettingsBody =
  components['schemas']['UpdateSystemSettingsRequest']
export type DockerReportDto = components['schemas']['DockerReportDto']
export type MultiUserDto = components['schemas']['MultiUserDto']
export type ScreenDto = components['schemas']['ScreenDto']
export type EnableMultiUserBody = components['schemas']['EnableMultiUserRequest']
export type AnalyticsDto = components['schemas']['AnalyticsDto']
export type ConnectionDto = components['schemas']['ConnectionDto']
export type ProviderEntryDto = components['schemas']['ProviderEntryDto']
export type ProviderFieldDto = components['schemas']['ProviderFieldDto']
export type ProviderSetupDto = components['schemas']['ProviderSetupDto']
export type SetupPartDto = components['schemas']['SetupPartDto']
export type CredentialDto = components['schemas']['CredentialDto']
export type PluginRowDto = components['schemas']['PluginRowDto']
export type PluginDto = components['schemas']['PluginDto']
export type PluginServerDto = components['schemas']['PluginServerDto']
export type PluginFieldDto = components['schemas']['PluginFieldDto']
export type PluginBindingDto = components['schemas']['PluginBindingDto']
export type PluginSourceRequest = components['schemas']['PluginSourceRequest']
export type BindingValueRequest = components['schemas']['BindingValueRequest']
export type CallDto = components['schemas']['CallDto']
export type CallSummaryDto = components['schemas']['CallSummaryDto']
export type TranscriptLineDto = components['schemas']['TranscriptLineDto']
export type SubscriptionDto = components['schemas']['SubscriptionDto']
export type WorkspaceDto = components['schemas']['WorkspaceDto']

export type ApiClient = ReturnType<typeof createClient<paths>>

export function createApiClient(): ApiClient {
  return createClient<paths>({ baseUrl: '/' })
}

/** One spoken block of an Agent message as audio bytes. The
 *  daemon synthesizes on every call and stores nothing. */
export async function fetchSpeech(
  channelId: string,
  messageId: string,
  block: number,
): Promise<Blob> {
  const response = await fetch(
    `/api/v1/channels/${channelId}/messages/${messageId}/speech?block=${block}`,
  )
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as {
      error?: { message?: string }
    } | null
    throw new Error(body?.error?.message ?? `speech failed (${response.status})`)
  }
  return response.blob()
}

/** Upload one file as an artifact. Duplicate content returns the
 *  stored original (same id), so re-pasting a screenshot is cheap. */
export async function uploadArtifact(file: File): Promise<ArtifactDto> {
  const form = new FormData()
  form.append('file', file, file.name)
  const response = await fetch('/api/v1/artifacts', {
    method: 'POST',
    body: form,
  })
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as {
      error?: { message?: string }
    } | null
    throw new Error(body?.error?.message ?? `upload failed (${response.status})`)
  }
  return (await response.json()) as ArtifactDto
}

/** The agent's screen preview as a Blob, or null when none
 *  exists yet. */
export async function fetchScreenPreview(agentId: string): Promise<Blob | null> {
  const response = await fetch(`/api/v1/agents/${agentId}/screen/preview.png`)
  if (response.status === 404) return null
  if (!response.ok) throw new Error(`preview failed (${response.status})`)
  return response.blob()
}

/** One Widget page (ADR-0016), fetched here and handed to the
 *  sandbox proxy over `postMessage`. The page is never the `src` of a
 *  frame: the sandbox is another origin, which the session cookie does
 *  not reach. */
export async function fetchWidgetPage(
  packageName: string,
  version: string,
  widget: string,
): Promise<string> {
  const path = [packageName, version, widget].map(encodeURIComponent).join('/')
  const response = await fetch(`/api/v1/widgets/${path}`)
  if (!response.ok) throw new Error(`widget page failed (${response.status})`)
  return response.text()
}

/** Fetch artifact bytes through the daemon (never from storage). */
export async function fetchArtifactBlob(artifactId: string): Promise<Blob> {
  const response = await fetch(`/api/v1/artifacts/${artifactId}`)
  if (!response.ok) throw new Error(`download failed (${response.status})`)
  return response.blob()
}
