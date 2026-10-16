import { workspacePath } from './workspace-path'
// The typed REST client: openapi-fetch over the generated schema. The
// session is an HTTP-only cookie, which the browser sends with
// every same-origin request, so no request carries a credential of its
// own. The schema is generated from the daemon's OpenAPI spec
// (`pnpm run api:generate`); CI fails on drift. File bytes go
// through plain fetch: multipart up, Blob down, always via the daemon.

import createClient from 'openapi-fetch'

import type { components, paths } from './schema'

export type WorkspaceNumberDto = components['schemas']['WorkspaceNumberDto']
export type ChannelDto = components['schemas']['ChannelDto']
export type MessageDto = components['schemas']['MessageDto']
export type ThreadDto = components['schemas']['ThreadDto']
export type TimelineItem = components['schemas']['TimelineItem']
export type PointerDto = components['schemas']['PointerDto']
export type ReplyAuthorDto = components['schemas']['ReplyAuthorDto']
export type EventRow = components['schemas']['EventRow']
export type DeltaFrame = components['schemas']['DeltaFrame']
export type ProgressFrame = components['schemas']['ProgressFrame']
export type FileDto = components['schemas']['FileDto']
export type GrantDto = components['schemas']['GrantDto']
export type SessionApprovalMode = components['schemas']['SessionApprovalMode']
export type CodingSessionDto = components['schemas']['CodingSessionDto']
export type CodingSessionEventDto = components['schemas']['CodingSessionEventDto']
export type CodingSessionUsage = components['schemas']['CodingSessionUsage']
export type SessionAllowRule = components['schemas']['SessionAllowRule']
export type HarnessDto = components['schemas']['HarnessDto']
export type HarnessSignInMethod = components['schemas']['HarnessSignInMethod']
export type MemoryFeedItem = components['schemas']['MemoryFeedItem']
export type AgentDto = components['schemas']['AgentDto']
export type ComputerDto = components['schemas']['ComputerDto']
export type RequestDto = components['schemas']['RequestDto']
export type KeypadCodeDto = components['schemas']['KeypadCodeDto']
export type RunDto = components['schemas']['RunDto']
export type NeedsYouItem = components['schemas']['NeedsYouItem']
export type NeedsYouQueue = components['schemas']['NeedsYouQueue']
export type RunTranscriptDto = components['schemas']['RunTranscriptDto']
export type RunEventDto = components['schemas']['RunEventDto']
export type RunStepsDto = components['schemas']['RunStepsDto']
export type RunStepDto = components['schemas']['RunStepDto']
export type ModelAliasDto = components['schemas']['ModelAliasDto']
export type RetentionPolicyDto = components['schemas']['RetentionPolicyDto']
export type PersonDto = components['schemas']['PersonDto']
export type WorkspaceUsageDto = components['schemas']['WorkspaceUsageDto']
export type InstallationUsageDto = components['schemas']['InstallationUsageDto']
export type MyUsageDto = components['schemas']['MyUsageDto']
export type SessionDto = components['schemas']['SessionDto']
export type MySessionDto = components['schemas']['MySessionDto']
export type SignInLinkDto = components['schemas']['SignInLinkDto']
export type PushSubscriptionDto = components['schemas']['PushSubscriptionDto']
export type PushOutcomeDto = components['schemas']['PushOutcomeDto']
export type NewPersonDto = components['schemas']['NewPersonDto']
export type HostDto = components['schemas']['HostDto']
export type HomeExitDto = components['schemas']['HomeExitDto']
export type HomeExitHostDto = components['schemas']['HomeExitHostDto']
export type SwitchedHomeExitDto = components['schemas']['SwitchedHomeExitDto']
export type HomeExitSettingDto = components['schemas']['HomeExitSettingDto']
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
export type ScreenDto = components['schemas']['ScreenDto']
export type RemoteAccessDto = components['schemas']['RemoteAccessDto']
export type TailscaleState = components['schemas']['TailscaleState']
export type SignInMethod = components['schemas']['SignInMethod']
export type AnalyticsDto = components['schemas']['AnalyticsDto']
export type ModelRequestCaptureSettingDto = components['schemas']['ModelRequestCaptureSettingDto']
export type RunModelRequestCaptureDto =
  components['schemas']['ModelRequestCapturesDto']['items'][number]
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
  const client = createClient<paths>({ baseUrl: '/' })
  client.use({
    onRequest({ request }) {
      const url = new URL(request.url)
      const path = workspacePath(url.pathname, undefined, request.method)
      if (path === url.pathname) return request
      url.pathname = path
      return new Request(url, request)
    },
  })
  return client
}

/** One spoken block of an Agent message as audio bytes. The
 *  daemon synthesizes on every call and stores nothing. */
export async function fetchSpeech(
  channelId: string,
  messageId: string,
  block: number,
): Promise<Blob> {
  const response = await fetch(
    workspacePath(`/api/v1/channels/${channelId}/messages/${messageId}/speech?block=${block}`),
  )
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as {
      error?: { message?: string }
    } | null
    throw new Error(body?.error?.message ?? `speech failed (${response.status})`)
  }
  return response.blob()
}

/** Upload one File. Duplicate content returns the
 *  stored original (same id), so re-pasting a screenshot is cheap. */
export async function uploadFile(file: File): Promise<FileDto> {
  const form = new FormData()
  form.append('file', file, file.name)
  const response = await fetch(workspacePath('/api/v1/files', undefined, 'POST'), {
    method: 'POST',
    body: form,
  })
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as {
      error?: { message?: string }
    } | null
    throw new Error(body?.error?.message ?? `upload failed (${response.status})`)
  }
  return (await response.json()) as FileDto
}

/** The agent's screen preview as a Blob, or null when none
 *  exists yet. */
export async function fetchScreenPreview(agentId: string): Promise<Blob | null> {
  const response = await fetch(workspacePath(`/api/v1/agents/${agentId}/screen/preview.png`))
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
  const response = await fetch(workspacePath(`/api/v1/widgets/${path}`))
  if (!response.ok) throw new Error(`widget page failed (${response.status})`)
  return response.text()
}

/** Fetch file bytes through the daemon (never from storage). */
export async function fetchFileBlob(fileId: string): Promise<Blob> {
  const response = await fetch(workspacePath(`/api/v1/files/${fileId}`))
  if (!response.ok) throw new Error(`download failed (${response.status})`)
  return response.blob()
}

export type SharedWorkspaceLimits = components['schemas']['SharedWorkspaceLimits']
export type MailVisibility = components['schemas']['MailVisibility']
export type AdministrationWorkspaceDto = components['schemas']['AdministrationWorkspaceDto']
