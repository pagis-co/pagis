// TanStack Query hooks over the typed REST client. Reads set an
// infinite staleTime because the WS event stream invalidates them; the
// send mutation is optimistic with `pending_id` reconciliation.

import { useEffect, useMemo, useState } from "react";
import {
  useInfiniteQuery,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
  QueryClient,
  type InfiniteData,
} from "@tanstack/react-query";
import type { components } from "./api/schema";
import { useCurrentWorkspaceId } from './workspace'
import { deviceTimezone } from "./timezone";

import type {
  ApiClient,
  AgentDto,
  BindingValueRequest,
  HarnessSignInMethod,
  MessageDto,
  NeedsYouQueue,
  PluginSourceRequest,
  SessionAllowRule,
  SessionApprovalMode,
  SystemSettingsBody,
  ThreadDto,
  TimelineItem,
} from "./api/client";
import { fetchScreenPreview, fetchWidgetPage } from "./api/client";
import type { ChangeKind } from "./components/memory/changes";
import { usePendingSends } from "./state/stores";
import { threadScope } from "./timeline";

export { useWorkspace } from "./workspace";

export const channelsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "channels"] as const;
export const timelineKey = (workspaceId: string, channelId: string) =>
  ["workspace", workspaceId, "timeline", channelId] as const;
export const threadKey = (workspaceId: string, channelId: string, rootId: string) =>
  ["workspace", workspaceId, "thread", channelId, rootId] as const;
export const onboardingKey = (workspaceId: string) =>
  ["workspace", workspaceId, "onboarding"] as const;
export const requestKey = (workspaceId: string, requestId: string) =>
  ["workspace", workspaceId, "request", requestId] as const;
export const grantsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "grants"] as const;
export const memoryFeedKey = (workspaceId: string) =>
  ["workspace", workspaceId, "memory-feed"] as const;
export const memoryPagesKey = (
  workspaceId: string,
  scope: string,
  kind: string | undefined,
  search: string,
) => ["workspace", workspaceId, "memory-pages", scope, "list", kind ?? "", search] as const;
export const memoryPageCountsKey = (workspaceId: string, scope: string) =>
  ["workspace", workspaceId, "memory-pages", scope, "counts"] as const;
export const memoryFileKey = (workspaceId: string, scope: string, path: string) =>
  ["workspace", workspaceId, "memory-file", scope, path] as const;
export const agentsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "agents"] as const;

export const userKey = ["user"] as const;
export const reportKey = (workspaceId: string) =>
  ["workspace", workspaceId, "report"] as const;
export const voicesKey = (workspaceId: string) =>
  ["workspace", workspaceId, "voices"] as const;
export const computerKey = (workspaceId: string, agentId: string) =>
  ["workspace", workspaceId, "computer", agentId] as const;
export const computerDiskKey = (workspaceId: string) =>
  ["workspace", workspaceId, "computers", "disk"] as const;
export const screenPreviewKey = (workspaceId: string, agentId: string) =>
  ["workspace", workspaceId, "screen-preview", agentId] as const;
export const runsKey = (workspaceId: string, agentId: string, channelId: string, state: string) =>
  ["workspace", workspaceId, "runs", agentId, channelId, state] as const;
export const runModelRequestsKey = (workspaceId: string, runId: string) =>
  ["workspace", workspaceId, "runs", runId, "model-requests"] as const;
export const runTranscriptKey = (workspaceId: string, runId: string) =>
  ["workspace", workspaceId, "run-transcript", runId] as const;
export const runStepsKey = (workspaceId: string, runId: string) =>
  ["workspace", workspaceId, "run-steps", runId] as const;
export const modelAliasesKey = (workspaceId: string) =>
  ["workspace", workspaceId, "model-aliases"] as const;
export const modelListsKey = ["model-lists"] as const;
export const providerSetupsKey = ["provider-setups"] as const;
export const retentionKey = (workspaceId: string) =>
  ["workspace", workspaceId, "retention"] as const;
export const systemSettingsKey = ["system-settings"] as const;
export const remoteAccessKey = ["system-settings", "remote-access"] as const;
export const peopleKey = ["administration", "people"] as const;
export const installationUsageKey = (period: string) =>
  ["administration", "usage", period] as const;
export const liveSessionsKey = ["administration", "sessions"] as const;
/** The signed-in person's own Sessions. */
export const mySessionsKey = ["sessions"] as const;
/** The public half of the VAPID Key, which a browser subscribes with. */
export const vapidKeyKey = ["push", "key"] as const;
/** The signed-in person's own Push Subscriptions. */
export const pushSubscriptionsKey = ["push-subscriptions"] as const;
/** The person's own machines. */
export const hostsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "hosts"] as const;
/** The Harness Catalog: the Coding Harnesses that Pagis starts. */
export const harnessesKey = ["harnesses"] as const;
/** The person's own Home Exit (ADR-0029). */
export const homeExitKey = (workspaceId: string) =>
  ["workspace", workspaceId, "home-exit"] as const;
/** Every machine of the installation, on the administration port. */
export const installationHostsKey = ["administration", "hosts"] as const;
export const resourcesKey = ["administration", "resources"] as const;
export const installationHealthKey = ["administration", "health"] as const;
export const setupKey = ["setup"] as const;
export const healthKey = ["health"] as const;
export const myUsageKey = (workspaceId: string) =>
  ["workspace", workspaceId, "usage"] as const;
export const connectionsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "connections"] as const;
export const connectionProvidersKey = ["connections", "providers"] as const;
export const credentialsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "credentials"] as const;
export const phoneNumbersKey = (workspaceId: string) =>
  ["workspace", workspaceId, "phone-numbers"] as const;
export const mailboxesKey = (workspaceId: string) =>
  ["workspace", workspaceId, "mailboxes"] as const;
export const agentMailboxKey = (workspaceId: string, agentId: string) =>
  ["workspace", workspaceId, "agent-mailbox", agentId] as const;
export const mailboxOffersKey = (agentName: string) =>
  ["mailbox-offers", agentName] as const;
export const mailboxNameKey = (connectionId: string, localPart: string) =>
  ["mailbox-name", connectionId, localPart] as const;
export const trustListKey = (workspaceId: string) =>
  ["workspace", workspaceId, "trust-list"] as const;
export const callKey = (workspaceId: string, callId: string) =>
  ["workspace", workspaceId, "call", callId] as const;
export const codingSessionsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "coding-sessions"] as const;
export const codingSessionKey = (workspaceId: string, sessionId: string) =>
  ["workspace", workspaceId, "coding-session", sessionId] as const;
export const codingSessionEventsKey = (workspaceId: string, sessionId: string) =>
  ["workspace", workspaceId, "coding-session-events", sessionId] as const;
export const mailMessageKey = (workspaceId: string, mailbox: string, messageId: string) =>
  ["workspace", workspaceId, "mail-message", mailbox, messageId] as const;
export const availableNumbersKey = (
  country: string,
  areaCode: string,
  locality: string,
) => ["available-numbers", country, areaCode, locality] as const;
export const widgetPageKey = (
  workspaceId: string,
  packageName: string,
  version: string,
  widget: string,
) => ["workspace", workspaceId, "widget-page", packageName, version, widget] as const;
export const widgetViewKey = (workspaceId: string, toolCallId: string) =>
  ["workspace", workspaceId, "widget-view", toolCallId] as const;
export const pendingRequestsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "requests", "pending"] as const;
export const needsYouKey = (workspaceId: string) =>
  ["workspace", workspaceId, "needs-you"] as const;
export const schedulesKey = (workspaceId: string) =>
  ["workspace", workspaceId, "schedules"] as const;
export const scheduleKey = (workspaceId: string, scheduleId: string) =>
  ["workspace", workspaceId, "schedule", scheduleId] as const;
export const scheduleOccurrencesKey = (workspaceId: string, scheduleId: string) =>
  ["workspace", workspaceId, "schedule-occurrences", scheduleId] as const;
export const scheduleWakeupsKey = (workspaceId: string, scheduleId: string) =>
  ["workspace", workspaceId, "schedule-wakeups", scheduleId] as const;
export const subscriptionsKey = (workspaceId: string) =>
  ["workspace", workspaceId, "event-subscriptions"] as const;
export const subscriptionKey = (workspaceId: string, subscriptionId: string) =>
  ["workspace", workspaceId, "event-subscription", subscriptionId] as const;
export const subscriptionEventsKey = (workspaceId: string, subscriptionId: string) =>
  ["workspace", workspaceId, "event-subscription-events", subscriptionId] as const;
export const subscriptionWakeupsKey = (workspaceId: string, subscriptionId: string) =>
  ["workspace", workspaceId, "event-subscription-wakeups", subscriptionId] as const;
export const pluginsKey = ["plugins"] as const;
export const pluginKey = (workspaceId: string, pluginId: string) =>
  ["workspace", workspaceId, "plugin", pluginId] as const;
export const pluginLogKey = (workspaceId: string, pluginId: string) =>
  ["workspace", workspaceId, "plugin-log", pluginId] as const;
export const softwareKey = (workspaceId: string) =>
  ["workspace", workspaceId, "software"] as const;
export const softwarePackageKey = (workspaceId: string, name: string) =>
  ["workspace", workspaceId, "software", name] as const;
export const contributionKey = (workspaceId: string, name: string, contributionId: string) =>
  ["workspace", workspaceId, "software-contribution", name, contributionId] as const;

/** One page of history in the Automations detail body (ADR-0006). */
const HISTORY_PAGE = 20;

/** The daemon's own words on a refusal. Every mutation here throws the
 *  `{error: {code, message}}` body the API returns, so one reader
 *  serves them all. */
export function errorMessage(error: unknown, fallback: string): string {
  const detail = (error as { error?: { message?: string } } | undefined)?.error;
  return detail?.message ?? fallback;
}

/** The daemon's stable error code, for a page that answers differently
 *  to `unauthorized` and to `forbidden`. `null` where the failure
 *  carries no code of the daemon's, such as a network error. */
export function errorCode(error: unknown): string | null {
  const detail = (error as { error?: { code?: string } } | undefined)?.error;
  return detail?.code ?? null;
}

/** The number of times a read asks again after a failure. It is the
 *  TanStack Query default. */
const READ_RETRIES = 3;

/** The query client of a page. A `not_found` is an answer, not a
 *  failure: the record does not exist, or it is not the person's, and
 *  a second request gets the same answer. So a read does not retry it,
 *  and the page says at once that the record does not exist. Other
 *  failures, such as a network error, retry. */
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: (failureCount, error) =>
          errorCode(error) !== "not_found" && failureCount < READ_RETRIES,
      },
    },
  });
}

async function unwrap<T>(
  request: Promise<{ data?: T; error?: unknown }>,
): Promise<T> {
  const { data, error } = await request;
  if (data === undefined) throw error ?? new Error("request failed");
  return data;
}

/** Wait for a request whose route answers 204 No Content. openapi-fetch
 *  answers it with no `data`, so `unwrap` would fail after the daemon
 *  did the work. This reads the status instead and answers nothing. */
async function expectNoContent(
  request: Promise<{ error?: unknown; response: Response }>,
): Promise<void> {
  const { error, response } = await request;
  if (error !== undefined || !response.ok)
    throw error ?? new Error("request failed");
}

const accountSyncKey = (workspaceId: string, id: string) =>
  ["workspace", workspaceId, "account-sync", id] as const;

export function useAccountSync(api: ApiClient, connectionId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: accountSyncKey(workspaceId, connectionId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/connections/{connection_id}/sync", {
          params: { path: { connection_id: connectionId } },
        }),
      ),
    refetchInterval: 5000,
  });
}

/** The Signal Catalogue of one resource and the filter it starts from
 *  (ADR-0011). The catalogue changes only when the account's labels do. */
export function useSyncCatalogue(api: ApiClient, connectionId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...accountSyncKey(workspaceId, connectionId), "catalogue"],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/connections/{connection_id}/sync/catalogue", {
          params: { path: { connection_id: connectionId } },
        }),
      ),
  });
}

/** The counts one filter gives over the stored pages of a resource:
 *  the What reflects section reads its live line from it. */
export function useFilterPreview(
  api: ApiClient,
  connectionId: string,
  filter: components["schemas"]["ReflectionFilter"] | undefined,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...accountSyncKey(workspaceId, connectionId), "preview", filter],
    queryFn: () =>
      unwrap(
        api.POST("/api/v1/connections/{connection_id}/sync/filter/preview", {
          params: { path: { connection_id: connectionId } },
          body: filter!,
        }),
      ),
    enabled: filter !== undefined,
  });
}

export function useConfigureSync(api: ApiClient, connectionId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: components["schemas"]["ConfigureSync"]) =>
      unwrap(
        api.PUT("/api/v1/connections/{connection_id}/sync", {
          params: { path: { connection_id: connectionId } },
          body,
        }),
      ),
    onSuccess: () =>
      client.resetQueries({ queryKey: accountSyncKey(workspaceId, connectionId) }),
  });
}

/** Onboarding status. No staleTime: Docker is probed lazily on
 *  the daemon, so a refetch notices Docker appearing. */
export function useOnboarding(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: onboardingKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/settings/onboarding")),
  });
}

/** Prove one provider's key with a call to its model list, which
 *  generates nothing and costs nothing. A passed check also refreshes
 *  the provider's list. */
export function useCheckProviderModel(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (provider: string) =>
      unwrap(
        api.POST("/api/v1/settings/providers/{provider}/check", {
          params: { path: { provider } },
        }),
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: onboardingKey(workspaceId) });
      void queryClient.invalidateQueries({ queryKey: modelListsKey });
    },
  });
}

/** The Provider Model List of every provider that holds a key, newest
 *  first, and the candidate a default pick takes. The daemon keeps the
 *  lists and refreshes them. */
export function useModelLists(api: ApiClient, enabled = true) {
  return useQuery({
    queryKey: modelListsKey,
    queryFn: () => unwrap(api.GET("/api/v1/settings/models")),
    enabled,
  });
}

/** The model step's pick: the one `provider/model` candidate the
 *  default route names. `candidate: null` takes the daemon's
 *  preselection. */
export function useSetOnboardingDefaultModel(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: { candidate: string | null }) => {
      const { error, response } = await api.PUT(
        "/api/v1/settings/onboarding/default-model",
        { body },
      );
      if (error !== undefined || !response.ok)
        throw error ?? new Error("save failed");
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: modelAliasesKey(workspaceId) });
    },
  });
}

/** The model step's check of a typed key. The daemon stores the key
 *  only when the provider lists its models for it. */
export function useCheckOnboardingProviderKey(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ provider, key }: { provider: string; key: string }) =>
      unwrap(
        api.POST("/api/v1/settings/onboarding/providers/{provider}/key/check", {
          params: { path: { provider } },
          body: { key },
        }),
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: onboardingKey(workspaceId) });
      void queryClient.invalidateQueries({ queryKey: modelListsKey });
    },
  });
}

/** The model step's key (ADR-0025). The product port takes it while
 *  onboarding runs and refuses it afterwards: the Administration
 *  Interface changes the installation's keys from then on. */
export function useSetOnboardingProviderKey(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ provider, key }: { provider: string; key: string }) =>
      unwrap(
        api.PUT("/api/v1/settings/onboarding/providers/{provider}/key", {
          params: { path: { provider } },
          body: { key },
        }),
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: onboardingKey(workspaceId) });
      void queryClient.invalidateQueries({ queryKey: modelListsKey });
      void queryClient.invalidateQueries({ queryKey: modelAliasesKey(workspaceId) });
    },
  });
}

/** "Check again" on the computer step. The onboarding read pings every
 *  candidate endpoint, so reading it again is the probe. */
export function useRecheckDocker() {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => queryClient.refetchQueries({ queryKey: onboardingKey(workspaceId) }),
  });
}

/** The computer step's Docker endpoint. Pagis pings it before it saves,
 *  and the product port takes it while onboarding runs alone. */
export function useSetOnboardingDockerEndpoint(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (dockerEndpoint: string | null) => {
      const { error, response } = await api.PUT(
        "/api/v1/settings/onboarding/docker-endpoint",
        { body: { docker_endpoint: dockerEndpoint } },
      );
      if (error !== undefined || !response.ok)
        throw error ?? new Error("save failed");
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: onboardingKey(workspaceId) });
    },
  });
}

export function useCompleteOnboarding(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ userName }: { userName?: string } = {}) => {
      const { error, response } = await api.POST(
        "/api/v1/settings/onboarding/complete",
        { body: { user_name: userName ?? null } },
      );
      if (error !== undefined || !response.ok) {
        throw error ?? new Error("request failed");
      }
    },
    // The wizard records the name, so the shell must read it again.
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: onboardingKey(workspaceId) });
      await queryClient.invalidateQueries({ queryKey: userKey });
    },
  });
}

export function useChannels(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: channelsKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/channels")).then((page) => page.items),
    staleTime: Infinity,
  });
}

/** Create a group channel with its agent participants. */
export function useCreateChannel(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ title, agentIds }: { title: string; agentIds: string[] }) =>
      unwrap(
        api.POST("/api/v1/channels", {
          body: { title, agent_ids: agentIds },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: channelsKey(workspaceId) }),
  });
}

/** The Provider Voice List: the model that speaks for the Workspace
 *  and its voices, the names an Agent Voice can take. It changes with
 *  the keys and the `speak` alias, so each form reads it again. */
export function useVoices(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: voicesKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/settings/voices")),
  });
}

/** Create an agent. The daemon also creates the agent's DM. */
export function useCreateAgent(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      name: string;
      job: string;
      description: string;
      personality: string;
      avatar: import("./avatars/catalog").SpriteAppearance;
      voice: string | null;
      mailbox?: NewMailboxBody;
    }) => unwrap(api.POST("/api/v1/agents", { body })),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: agentsKey(workspaceId) });
      void queryClient.invalidateQueries({ queryKey: channelsKey(workspaceId) });
    },
  });
}

/** Rewrite an agent's profile. */
export function useUpdateAgent(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      agentId,
      ...body
    }: {
      agentId: string;
      name: string;
      job: string;
      description: string;
      personality: string;
      voice: string | null;
      /** What a Call the Agent answers is for (ADR-0020);
       *  `null` leaves the Agent taking a message. */
      standing_brief?: string | null;
    }) =>
      unwrap(
        api.PUT("/api/v1/agents/{agent_id}", {
          params: { path: { agent_id: agentId } },
          body,
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: agentsKey(workspaceId) }),
  });
}

/** Save appearance without replacing the About fields. */
export function useUpdateAppearance(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      agentId,
      avatar,
    }: {
      agentId: string;
      avatar: import("./avatars/catalog").SpriteAppearance;
    }) =>
      unwrap(
        api.PUT("/api/v1/agents/{agent_id}/appearance", {
          params: { path: { agent_id: agentId } },
          body: avatar,
        }),
      ),
    onSuccess: (saved) => {
      queryClient.setQueryData<AgentDto[]>(agentsKey(workspaceId), (agents) =>
        agents?.map((agent) => (agent.id === saved.id ? saved : agent)),
      );
      void queryClient.invalidateQueries({ queryKey: agentsKey(workspaceId) });
    },
  });
}

/** Archive an agent: it stops triggering; history stays. */
export function useArchiveAgent(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (agentId: string) =>
      unwrap(
        api.POST("/api/v1/agents/{agent_id}/archive", {
          params: { path: { agent_id: agentId } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: agentsKey(workspaceId) }),
  });
}

// Conversations cross Workspaces, so these keys have no Workspace id.
export const colleaguesKey = ["colleagues"] as const;
export const conversationsKey = ["conversations"] as const;
export const conversationMessagesKey = (conversationId: string) =>
  [...conversationsKey, conversationId, "messages"] as const;

export function useColleagues(api: ApiClient) {
  return useQuery({
    queryKey: colleaguesKey,
    queryFn: () =>
      unwrap(api.GET("/api/v1/colleagues")).then((page) => page.items),
    refetchInterval: 30_000,
  });
}

export function useConversations(api: ApiClient) {
  return useQuery({
    queryKey: conversationsKey,
    queryFn: () =>
      unwrap(api.GET("/api/v1/conversations")).then((page) => page.items),
    staleTime: Infinity,
  });
}

export function useStartConversation(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (workspaceIds: string[]) =>
      unwrap(
        api.POST("/api/v1/conversations", {
          body: { workspace_ids: workspaceIds },
        }),
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: conversationsKey });
    },
  });
}

export function useConversationMessages(
  api: ApiClient,
  conversationId: string,
) {
  return useInfiniteQuery({
    queryKey: conversationMessagesKey(conversationId),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/conversations/{conversation_id}/messages", {
          params: {
            path: { conversation_id: conversationId },
            query: { before: pageParam },
          },
        }),
      ),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    staleTime: Infinity,
  });
}

export function useSendConversationMessage(
  api: ApiClient,
  conversationId: string,
) {
  const queryClient = useQueryClient();
  const pending = usePendingSends();
  return useMutation({
    mutationFn: ({ pendingId, text, forPersonOnly = false, parentMessageId }: { pendingId: string; text: string; forPersonOnly?: boolean; parentMessageId?: string }) =>
      unwrap(
        api.POST("/api/v1/conversations/{conversation_id}/messages", {
          params: { path: { conversation_id: conversationId } },
          body: { text, pending_id: pendingId, for_person_only: forPersonOnly, parent_message_id: parentMessageId },
        }),
      ),
    onMutate: ({ pendingId, text, forPersonOnly = false, parentMessageId }) => {
      const existing = usePendingSends
        .getState()
        .byScope[conversationId]?.some((send) => send.pending_id === pendingId);
      if (existing) {
        pending.markPending(conversationId, pendingId);
      } else {
        pending.add(conversationId, {
          for_person_only: forPersonOnly,
          parent_message_id: parentMessageId,
          pending_id: pendingId,
          text,
          created_at: Date.now(),
          state: "pending",
        });
      }
    },
    onSuccess: (message, { pendingId }) => {
      queryClient.setQueryData<
        InfiniteData<components["schemas"]["ConversationMessages"]>
      >(conversationMessagesKey(conversationId), (data) => {
        if (
          data === undefined ||
          data.pages.some((page) =>
            page.items.some((item) => item.id === message.id),
          )
        )
          return data;
        return {
          ...data,
          pages: data.pages.map((page, index) =>
            index === 0 ? { ...page, items: [message, ...page.items] } : page,
          ),
        };
      });
      pending.remove(conversationId, pendingId);
      void queryClient.invalidateQueries({ queryKey: conversationsKey });
    },
    onError: (_error, { pendingId }) => {
      pending.markFailed(conversationId, pendingId);
    },
  });
}

/** The top-level messages of a conversation, newest first. The
 *  daemon answers a conversation that is not the person's as not
 *  found, and the query client does not retry that answer. */
export function useTimeline(api: ApiClient, channelId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: timelineKey(workspaceId, channelId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/channels/{channel_id}/messages", {
          params: { path: { channel_id: channelId } },
        }),
      ).then((page) => page.items),
    staleTime: Infinity,
  });
}

/** One thread: the root plus its replies, oldest first. */
export function useThread(api: ApiClient, channelId: string, rootId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: threadKey(workspaceId, channelId, rootId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/channels/{channel_id}/threads/{root_message_id}", {
          params: {
            path: { channel_id: channelId, root_message_id: rootId },
          },
        }),
      ),
    staleTime: Infinity,
  });
}

/** Insert a confirmed top-level message unless the timeline has it. */
export function insertMessage(
  workspaceId: string,
  queryClient: QueryClient,
  message: MessageDto,
): void {
  queryClient.setQueryData<TimelineItem[]>(
    timelineKey(workspaceId, message.channel_id),
    (items) => {
      if (items === undefined) return items;
      if (items.some((item) => item.id === message.id)) return items;
      // The page is newest first. A fresh message has no replies.
      return [
        {
          kind: "message",
          reply_count: 0,
          last_reply_at: null,
          reply_authors: [],
          ...message,
        },
        ...items,
      ];
    },
  );
}

/** Append a confirmed reply unless the thread already has it. */
export function insertReply(
  workspaceId: string,
  queryClient: QueryClient,
  rootId: string,
  message: MessageDto,
): void {
  queryClient.setQueryData<ThreadDto>(
    threadKey(workspaceId, message.channel_id, rootId),
    (thread) => {
      if (thread === undefined) return thread;
      if (thread.replies.some((reply) => reply.id === message.id))
        return thread;
      return { ...thread, replies: [...thread.replies, message] };
    },
  );
}

/** Cancel a run: the daemon stops the run and the partial persists
 *  as `failed`. The settled progress row of the run ends the Working
 *  row. */
export function useCancelRun(api: ApiClient) {
  return useMutation({
    mutationFn: (runId: string) =>
      unwrap(
        api.POST("/api/v1/runs/{run_id}/cancel", {
          params: { path: { run_id: runId } },
        }),
      ),
  });
}

/** The Request row behind a card: the state's source of
 *  truth. The `request.decided` WS event invalidates it, so every card
 *  of one Request re-renders together. */
export function useRequest(api: ApiClient, requestId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: requestKey(workspaceId, requestId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/requests/{request_id}", {
          params: { path: { request_id: requestId } },
        }),
      ),
    staleTime: Infinity,
  });
}

/** Decide a Request. `scope: 'always'` also stores the proposed Allow
 *  Rule or Disclosure Rule. A form or a choice submits `values`. Any
 *  failure (409 on a lost race, 422 on values the row's schema
 *  refuses) refetches the row so the card shows the real state. */
export function useDecideRequest(api: ApiClient, requestId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      decision,
      scope,
      values,
    }: {
      decision: "approved" | "denied";
      scope?: "once" | "always";
      values?: Record<string, unknown>;
    }) =>
      unwrap(
        api.POST("/api/v1/requests/{request_id}/decision", {
          params: { path: { request_id: requestId } },
          body: { decision, scope, values },
        }),
      ),
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: requestKey(workspaceId, requestId) }),
  });
}

/** The live grants with their allow rules, for the settings page.
 *  The `grant.changed` / `grant.revoked` WS events invalidate it. */
export function useGrants(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: grantsKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/grants")).then((page) => page.items),
    staleTime: Infinity,
  });
}

/** The person's own machines, with whether each is connected now.
 *  No staleTime: presence is what the list is for, and a machine goes
 *  away the moment its client does. */
export function useHosts(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: hostsKey(workspaceId),
    queryFn: async () => (await unwrap(api.GET("/api/v1/hosts"))).items,
  });
}

/** The Harness Catalog: each Coding Harness with its sign-in methods.
 *  The catalog is part of the daemon's build, so it never goes stale.
 *  The sign-in report of each harness on a Host comes with each Host in
 *  `useHosts`. */
export function useHarnesses(api: ApiClient) {
  return useQuery({
    queryKey: harnessesKey,
    queryFn: async () => (await unwrap(api.GET("/api/v1/harnesses"))).items,
    staleTime: Infinity,
  });
}

/** Start a Harness Sign-In of one harness on one Host. The route answers
 *  when the Client App has it, and the Person signs in in the terminal
 *  window that it opens (ADR-0033). */
export function useStartHarnessSignIn(api: ApiClient) {
  return useMutation({
    mutationFn: ({
      hostId,
      harnessId,
      method,
    }: {
      hostId: string;
      harnessId: string;
      method: HarnessSignInMethod;
    }) =>
      unwrap(
        api.POST("/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in", {
          params: { path: { host_id: hostId, harness_id: harnessId } },
          body: { method },
        }),
      ),
  });
}

/** Ask a Host to run the vendor's status command of one harness. The
 *  route answers when the Client App has the check, and the state comes
 *  with the Host after `harness.sign_in_changed` (ADR-0033). */
export function useCheckHarnessSignIn(api: ApiClient) {
  return useMutation({
    mutationFn: ({ hostId, harnessId }: { hostId: string; harnessId: string }) =>
      expectNoContent(
        api.POST("/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in-check", {
          params: { path: { host_id: hostId, harness_id: harnessId } },
        }),
      ),
  });
}

/** The person's own Home Exit on a server (ADR-0029): the Host they
 *  chose, the Hosts they can choose, and whether the Administrator
 *  turned it off. A local installation answers that it has none. */
export function useHomeExit(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: homeExitKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/settings/home-exit")),
  });
}

/** Choose one Host as the Home Exit. The daemon switches each awake
 *  Computer of the person at once, and names each one that did not. */
export function useChooseHomeExit(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (hostId: string) =>
      unwrap(api.PUT("/api/v1/settings/home-exit", { body: { host_id: hostId } })),
    onSuccess: (saved) => queryClient.setQueryData(homeExitKey(workspaceId), saved.home_exit),
  });
}

/** Turn the Home Exit off: the person's Computers reach the internet
 *  from the server again, at once. */
export function useTurnOffHomeExit(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.DELETE("/api/v1/settings/home-exit")),
    onSuccess: (saved) => queryClient.setQueryData(homeExitKey(workspaceId), saved.home_exit),
  });
}

/** Every machine of the installation, with the person who owns it.
 *  Administrator only, on the administration port. */
export function useInstallationHosts(api: ApiClient) {
  return useQuery({
    queryKey: installationHostsKey,
    queryFn: async () =>
      (await unwrap(api.GET("/api/v1/administration/hosts"))).items,
  });
}

/** Give one Agent named capabilities on one Connection. */
export function useCreateConnectionGrant(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      agent_id: string;
      connection_id: string;
      capabilities: string[];
    }) => unwrap(api.POST("/api/v1/grants", { body })),
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  });
}

/** Give one Agent outbound Call access to the Workspace Number. */
export function useCreateWorkspaceMailboxGrant(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { agent_id: string }) => unwrap(api.POST('/api/v1/grants/workspace-mailbox', { body })),
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  });
}

export function useCreateWorkspaceNumberGrant(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId()
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: {
      agent_id: string
      number_id: string
    }) =>
      unwrap(api.POST('/api/v1/grants/workspace-number', { body })),
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  })
}

/** Replace the complete named capability set on a Connection grant. */
export function useSetGrantCapabilities(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      grantId,
      capabilities,
    }: {
      grantId: string;
      capabilities: string[];
    }) =>
      unwrap(
        api.PUT("/api/v1/grants/{grant_id}/capabilities", {
          params: { path: { grant_id: grantId } },
          body: { capabilities },
        }),
      ),
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  });
}

/** Replace one grant's allow rules, and the session allow rules of a
 *  host grant when `sessions` is given. Without it they stay. */
export function useSetGrantRules(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      grantId,
      allow,
      sessions,
    }: {
      grantId: string;
      allow: string[];
      sessions?: SessionAllowRule[];
    }) =>
      unwrap(
        api.PUT("/api/v1/grants/{grant_id}/rules", {
          params: { path: { grant_id: grantId } },
          body: sessions === undefined ? { allow } : { allow, sessions },
        }),
      ),
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  });
}

/** Set the widest Session Approval Mode of one Agent on one machine.
 *  The host Grant holds it, and the first write on a machine with no
 *  Grant makes one. */
export function useSetSessionApprovalMode(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      agentId,
      hostId,
      mode,
    }: {
      agentId: string;
      hostId: string;
      mode: SessionApprovalMode;
    }) =>
      unwrap(
        api.PUT(
          "/api/v1/agents/{agent_id}/hosts/{host_id}/session-approval-mode",
          {
            params: { path: { agent_id: agentId, host_id: hostId } },
            body: { mode },
          },
        ),
      ),
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  });
}

/** Allow or forbid Unattended Modes for one Agent on one machine. The
 *  host Grant holds the allowance, and the first write on a machine with
 *  no Grant makes one. */
export function useSetUnattendedModes(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      agentId,
      hostId,
      allowed,
    }: {
      agentId: string;
      hostId: string;
      allowed: boolean;
    }) =>
      unwrap(
        api.PUT("/api/v1/agents/{agent_id}/hosts/{host_id}/unattended-modes", {
          params: { path: { agent_id: agentId, host_id: hostId } },
          body: { allowed },
        }),
      ),
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  });
}

/** Revoke a grant: the off switch. The next host call asks again. */
export function useRevokeGrant(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (grantId: string) => {
      const { error, response } = await api.DELETE(
        "/api/v1/grants/{grant_id}",
        {
          params: { path: { grant_id: grantId } },
        },
      );
      if (error !== undefined || !response.ok) {
        throw error ?? new Error("request failed");
      }
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  });
}

/** The Changes view of one scope: commits and reverts, newest
 *  first, narrowed to one source kind. The key sits under
 *  `memoryFeedKey`, so a memory event refreshes it. */
export function useMemoryChanges(
  api: ApiClient,
  scope: string,
  kind?: ChangeKind,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...memoryFeedKey(workspaceId), "changes", scope, kind ?? "all"],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/memory/feed", {
          params: { query: { scope, ...(kind === undefined ? {} : { kind }) } },
        }),
      ),
    staleTime: Infinity,
  });
}

/** The lines before and after of one commit. A commit does not
 *  change, so the answer never goes stale. */
export function useCommitDiff(api: ApiClient, sha: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: ["workspace", workspaceId, "memory-diff", sha],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/memory/commits/{sha}/diff", {
          params: { path: { sha } },
        }),
      ),
    staleTime: Infinity,
  });
}

/** The newest memory feed of the workspace. The learned line
 *  finds the commits of one reply in it. */
export function useMemoryFeed(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: memoryFeedKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/memory/feed")),
    staleTime: Infinity,
  });
}

/** One-tap revert. A success refreshes the feed, the pages and
 *  the open files. A 409 `revert_conflict` carries the guidance
 *  message; the caller shows it inline. */
export function useRevertCommit(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      sha,
      expectedRevision,
    }: {
      sha: string;
      expectedRevision: string;
    }) => {
      const { data, error } = await api.POST(
        "/api/v1/memory/commits/{sha}/revert",
        {
          params: { path: { sha } },
          body: { expected_revision: expectedRevision },
        },
      );
      if (data === undefined) {
        const detail = (error as { error?: { message?: string } } | undefined)
          ?.error;
        throw new Error(detail?.message ?? "revert failed");
      }
      return data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: memoryFeedKey(workspaceId) });
      void queryClient.invalidateQueries({ queryKey: ["workspace", workspaceId, "memory-pages"] });
      void queryClient.resetQueries({ queryKey: ["workspace", workspaceId, "memory-file"] });
    },
  });
}

/** The pages of one scope, `agent:<id>` or `shared`, newest change
 *  first. The list comes in parts: the daemon narrows it
 *  by the kind and the search, and each part names the full count. */
export function useMemoryPages(
  api: ApiClient,
  scope: string,
  kind: string | undefined,
  search: string,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useInfiniteQuery({
    queryKey: memoryPagesKey(workspaceId, scope, kind, search),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/memory/pages", {
          params: {
            query: {
              scope,
              ...(kind === undefined ? {} : { kind }),
              ...(search === "" ? {} : { q: search }),
              ...(pageParam === null ? {} : { after: pageParam }),
            },
          },
        }),
      ),
    getNextPageParam: (part) => part.next ?? undefined,
    // The rows stay while the next search loads.
    placeholderData: (previous, query) =>
      query?.queryKey[1] === workspaceId ? previous : undefined,
    staleTime: Infinity,
  });
}

/** How many pages a scope holds, and who changed them last. */
export function useMemoryPageCounts(api: ApiClient, scope: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: memoryPageCountsKey(workspaceId, scope),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/memory/pages/counts", {
          params: { query: { scope } },
        }),
      ),
    staleTime: Infinity,
  });
}

/** The feed entries that touch one page, newest first. The key
 *  sits under `memoryFeedKey`, so a memory event refreshes it too. */
export function usePageHistory(api: ApiClient, scope: string, path: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...memoryFeedKey(workspaceId), scope, path],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/memory/feed", { params: { query: { scope, path } } }),
      ),
    staleTime: Infinity,
  });
}

/** The newest feed entries of one scope. The key sits under
 *  `memoryFeedKey`, so a memory event refreshes it too. */
export function useRecentChanges(api: ApiClient, scope: string, limit: number) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...memoryFeedKey(workspaceId), scope, "recent", limit],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/memory/feed", { params: { query: { scope, limit } } }),
      ),
    staleTime: Infinity,
  });
}

/** The connections whose sync names one Agent as responsible. */
export function useAgentSyncConnections(api: ApiClient, agentId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: ["workspace", workspaceId, "agent-sync-connections", agentId],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/agents/{agent_id}/sync-connections", {
          params: { path: { agent_id: agentId } },
        }),
      ).then((list) => list.items),
  });
}

/** One fact file, read-only. */
export function useMemoryFile(api: ApiClient, scope: string, path: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: memoryFileKey(workspaceId, scope, path),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/memory/file", {
          params: { query: { scope, path } },
        }),
      ),
    staleTime: Infinity,
  });
}

/** The person the daemon serves. The session cookie names
 *  them, so a refused answer means no session: the app reads this
 *  query to decide between the sign-in page and itself. A 401 is an
 *  answer, not a failure, so the query does not retry. */
export function useUser(api: ApiClient) {
  return useQuery({
    queryKey: userKey,
    queryFn: () => unwrap(api.GET("/api/v1/user")),
    staleTime: Infinity,
    retry: false,
  });
}

/** Whether the person is the installation's administrator. A
 *  System Setting belongs to the installation, so a member neither sees
 *  the System section nor opens its address. An answer that has not
 *  arrived yet is not an administrator, which is the safe reading. */
export function useIsAdministrator(api: ApiClient): boolean {
  return useUser(api).data?.role === "administrator";
}

/** The name to show the person, in every place that shows them. Until
 *  onboarding records a name, their address stands for it; the seeded
 *  person of a local installation has neither and reads as "You". */
export function useUserName(api: ApiClient): string {
  const user = useUser(api).data;
  return user?.name ?? user?.email ?? "You";
}

/** Sign a person in with their address and their password. The
 *  daemon answers with an HTTP-only cookie, which the browser sends
 *  from now on; nothing here holds a credential. */
export function useSignIn(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (credential: { email: string; password: string }) => {
      // The daemon takes the device's timezone at the Person's first
      // sign-in, so their Schedules run on their own clock.
      const { data, error, response } = await api.POST("/api/v1/sessions", {
        body: { ...credential, timezone: deviceTimezone() },
      });
      if (data === undefined) throw new Error(signInMessage(response.status, error));
      return data;
    },
    onSuccess: (user) => queryClient.setQueryData(userKey, user),
  });
}

/** What the sign-in page tells a person whose sign-in was refused. */
function signInMessage(status: number, error: unknown): string {
  if (status === 401) return "The address and the password do not match.";
  // In Remote Access the daemon takes no password from another machine,
  // and its answer names the Sign-In Link.
  if (status === 403) {
    return errorMessage(error, "This Pagis takes no password from this machine.");
  }
  if (status === 429) {
    return "Too many attempts. Wait a minute, then sign in again.";
  }
  return `Sign-in failed (${status}). Try again.`;
}

/** Trade the secret of a Sign-In Link for a Session. The page at
 *  `/sign-in` posts it, so opening the link spends nothing. The daemon
 *  answers with an HTTP-only cookie, as a password sign-in does. */
export function useLinkSignIn(api: ApiClient) {
  return useMutation({
    mutationFn: async ({ secret, verifier }: { secret: string; verifier?: string }) => {
      const { data, error, response } = await api.POST("/api/v1/sessions/link", {
        body: { secret, ...(verifier === undefined ? {} : { verifier }), timezone: deviceTimezone() },
      });
      if (data === undefined) throw new Error(linkSignInMessage(response.status, error));
      return data;
    },
  });
}

/** Where a person gets a new Sign-In Link, in the words of the daemon's
 *  refusal of a spent or expired link. */
export const WHERE_TO_GET_A_LINK =
  "Make a new link in Settings → Sessions on a browser or app that is signed in. Or ask " +
  'an Administrator for a new invite, or run "pagis pair" on the machine of the server.';

/** What the sign-in page tells a person whose link was refused. The
 *  daemon's refusal of a spent or expired link names the ways to a new
 *  one, so the page shows it as it is, and says the same where the
 *  answer holds no refusal. */
function linkSignInMessage(status: number, error: unknown): string {
  if (status === 401) {
    return errorMessage(
      error,
      `This sign-in link is spent or expired. ${WHERE_TO_GET_A_LINK}`,
    );
  }
  if (status === 429) {
    return "Too many attempts. Wait a few minutes, then open the link again.";
  }
  return `Sign-in failed (${status}). Open the link again.`;
}

/** The signed-in person's own Sessions: each browser and app that is
 *  signed in as them, and which one is this one. */
export function useMySessions(api: ApiClient) {
  return useQuery({
    queryKey: mySessionsKey,
    queryFn: async () =>
      (await unwrap(api.GET("/api/v1/settings/sessions"))).items,
  });
}

/** End one of the person's own Sessions. Its browser or app is signed
 *  out at once. */
export function useEndMySession(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (sessionId: string) =>
      expectNoContent(
        api.DELETE("/api/v1/settings/sessions/{session_id}", {
          params: { path: { session_id: sessionId } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: mySessionsKey }),
  });
}

/** The public half of the VAPID Key: the `applicationServerKey` that
 *  `PushManager.subscribe` takes. It does not change, so it is read once. */
export function useVapidKey(api: ApiClient, enabled: boolean) {
  return useQuery({
    queryKey: vapidKeyKey,
    queryFn: async () =>
      (await unwrap(api.GET("/api/v1/push/key"))).vapid_public_key,
    enabled,
    staleTime: Infinity,
  });
}

/** The signed-in person's own Push Subscriptions, each named by the
 *  client of its Session, with the one of this Session marked. */
export function usePushSubscriptions(api: ApiClient) {
  return useQuery({
    queryKey: pushSubscriptionsKey,
    queryFn: async () =>
      (await unwrap(api.GET("/api/v1/push-subscriptions"))).items,
  });
}

/** Give the daemon the Push Subscription of this browser: the body of
 *  `PushSubscription.toJSON()`. */
export function useSubscribeToPush(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: components["schemas"]["SubscribeRequest"]) =>
      unwrap(api.POST("/api/v1/push-subscriptions", { body })),
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: pushSubscriptionsKey }),
  });
}

/** Remove one of the person's own Push Subscriptions. */
export function useRemovePushSubscription(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (pushSubscriptionId: string) =>
      expectNoContent(
        api.DELETE("/api/v1/push-subscriptions/{push_subscription_id}", {
          params: { path: { push_subscription_id: pushSubscriptionId } },
        }),
      ),
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: pushSubscriptionsKey }),
  });
}

/** Send a test Notification to one Push Subscription. The answer is
 *  what the push service answered; a `gone` answer means the daemon
 *  deleted the Push Subscription, so the list is read again. */
export function useSendTestNotification(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (pushSubscriptionId: string) =>
      unwrap(
        api.POST("/api/v1/push-subscriptions/{push_subscription_id}/test", {
          params: { path: { push_subscription_id: pushSubscriptionId } },
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: pushSubscriptionsKey }),
  });
}

/** Make a Sign-In Link for one more browser or app of the signed-in
 *  person. It is good for five minutes and one use. */
export function useMakeSignInLink(api: ApiClient) {
  return useMutation({
    mutationFn: () => unwrap(api.POST("/api/v1/settings/sign-in-links")),
  });
}

/** End the session. The daemon clears the cookie; every cached
 *  answer belonged to the session, so every query drops its data and
 *  reads again. The read of the person is then refused, which gives
 *  the screen back to the sign-in page. */
export function useSignOut(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      await api.DELETE("/api/v1/sessions/current");
    },
    onSuccess: () => queryClient.resetQueries(),
  });
}

/** Make an IANA timezone the Person's own. The Daily report moves with
 *  it, so the Report's next time is read again. */
export function useSetTimezone(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (timezone: string) =>
      unwrap(api.PUT("/api/v1/workspace/timezone", { body: { timezone } })),
    onSuccess: (saved) => {
      queryClient.setQueryData(['workspace', workspaceId, 'details'], saved)
      void queryClient.invalidateQueries({ queryKey: reportKey(workspaceId) });
    },
  });
}

/** Move the Chief of Staff designation from the Staff roster. */
export function useSetChiefOfStaff(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (agentId: string) =>
      unwrap(
        api.PUT("/api/v1/workspace/chief-of-staff", {
          body: { agent_id: agentId },
        }),
      ),
    onSuccess: (saved) => queryClient.setQueryData(['workspace', workspaceId, 'details'], saved),
  });
}

/** The Report that Home reads (ADR-0022): the newest one written, and
 *  the Run writing one now. */
export function useReport(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: reportKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/workspace/report")),
  });
}

/** Ask a Schedule to run now, ahead of its cadence. Home writes a
 *  Report this way. */
export function useRunScheduleNow(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (scheduleId: string) =>
      unwrap(
        api.POST("/api/v1/schedules/{schedule_id}/run", {
          params: { path: { schedule_id: scheduleId } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: reportKey(workspaceId) }),
  });
}

/** The agent roster. */
export function useAgents(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: agentsKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/agents")).then((page) => page.items),
    staleTime: Infinity,
  });
}

/** The roster by id: the name a message row shows for its
 *  author. An agent the roster no longer holds keeps the plain label. */
export function useAgentNames(api: ApiClient): Record<string, string> {
  const agents = useAgents(api);
  return useMemo(() => {
    const names: Record<string, string> = {};
    for (const agent of agents.data ?? []) names[agent.id] = agent.name;
    return names;
  }, [agents.data]);
}

export function useRuns(
  api: ApiClient,
  agentId: string,
  channelId: string,
  state: string,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: runsKey(workspaceId, agentId, channelId, state),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/runs", {
          params: {
            query: {
              agent_id: agentId || undefined,
              channel_id: channelId || undefined,
              state: state || undefined,
            },
          },
        }),
      ).then((page) => page.items),
  });
}

/** The Model Request Captures of a Run (ADR-0031). Only an
 *  Administrator reads them, so a Member's page never asks. */
export function useRunModelRequests(api: ApiClient, runId: string, enabled: boolean) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: runModelRequestsKey(workspaceId, runId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/runs/{run_id}/model-requests", {
          params: { path: { run_id: runId } },
        }),
      ),
    enabled,
  });
}

export function useRunTranscript(api: ApiClient, runId: string | null) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: runTranscriptKey(workspaceId, runId ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/runs/{run_id}/events", {
          params: { path: { run_id: runId! } },
        }),
      ),
    enabled: runId !== null,
  });
}

/** The reader dismisses a failed Run from the Needs-You Queue. The
 *  item leaves the cached queue at once, before the daemon answers; the
 *  `needs_you.removed` WS event refreshes the other clients. */
export function useDismissRun(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (runId: string) =>
      expectNoContent(
        api.POST("/api/v1/runs/{run_id}/dismiss", {
          params: { path: { run_id: runId } },
        }),
      ),
    onMutate: (runId) => dropNeedsYouItem(workspaceId, queryClient, `run:${runId}`),
    onSettled: () => queryClient.invalidateQueries({ queryKey: needsYouKey(workspaceId) }),
  });
}

/** Queue the same durable evidence after a memory review failed. */
export function useRetryReview(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (runId: string) =>
      unwrap(
        api.POST("/api/v1/runs/{run_id}/retry", {
          params: { path: { run_id: runId } },
        }),
      ),
    onSuccess: (_result, runId) => {
      void queryClient.invalidateQueries({ queryKey: ["workspace", workspaceId, "runs"] });
      void queryClient.invalidateQueries({ queryKey: runTranscriptKey(workspaceId, runId) });
    },
  });
}

/** The steps of one Run: the work record, the Working row and
 *  the quiet Stopped and Failed lines read them. A finished Run never
 *  changes, so the answer stays fresh; the shell drops the cache of a
 *  Run on each of its events, so a live Run stays current. */
export function useRunSteps(api: ApiClient, runId: string | null) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: runStepsKey(workspaceId, runId ?? ""),
    staleTime: Infinity,
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/runs/{run_id}/steps", {
          params: { path: { run_id: runId! } },
        }),
      ),
    enabled: runId !== null,
  });
}

export function useModelAliases(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: modelAliasesKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/settings/model-aliases")).then(
        (page) => page.items,
      ),
  });
}

export function useCreateModelAlias(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      alias,
      candidates,
    }: {
      alias: string;
      candidates: string[];
    }) =>
      unwrap(
        api.POST("/api/v1/settings/model-aliases", {
          body: { alias, candidates },
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: modelAliasesKey(workspaceId) }),
  });
}

export function useUpdateModelAlias(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      alias,
      candidates,
    }: {
      alias: string;
      candidates: string[];
    }) =>
      unwrap(
        api.PUT("/api/v1/settings/model-aliases/{alias}", {
          params: { path: { alias } },
          body: { candidates },
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: modelAliasesKey(workspaceId) }),
  });
}

export function useDeleteModelAlias(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (alias: string) => {
      const { error, response } = await api.DELETE(
        "/api/v1/settings/model-aliases/{alias}",
        { params: { path: { alias } } },
      );
      if (error !== undefined || !response.ok)
        throw error ?? new Error("delete failed");
    },
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: modelAliasesKey(workspaceId) }),
  });
}

/** The installation's setup of every provider, on the administration
 *  port: the model keys, the Google OAuth client, the carrier account
 *  with its SIP sign-in and the mail domain. One set of routes serves
 *  each part every provider declares. */
export function useProviderSetups(api: ApiClient) {
  return useQuery({
    queryKey: providerSetupsKey,
    queryFn: () =>
      unwrap(api.GET("/api/v1/administration/providers")).then(
        (page) => page.items,
      ),
  });
}

/** One part of one provider, as the administration routes name it. */
export type SetupPartRef = { provider: string; part: string };

/** What a change to a provider's setup makes stale: the setups, and the
 *  person's side that reads what the installation set up. */
function invalidateProviderSetups(queryClient: QueryClient) {
  void queryClient.invalidateQueries({ queryKey: providerSetupsKey });
  void queryClient.invalidateQueries({
    predicate: ({ queryKey }) => queryKey[0] === "workspace" &&
      ["onboarding", "connections", "phone-numbers"].includes(queryKey[2] as string),
  });
}

export function useConfigureProviderPart(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      provider,
      part,
      fields,
    }: SetupPartRef & { fields: Record<string, string> }) =>
      unwrap(
        api.PUT("/api/v1/administration/providers/{provider}/{part}", {
          params: { path: { provider, part } },
          body: { fields },
        }),
      ),
    onSuccess: () => invalidateProviderSetups(queryClient),
  });
}

export function useTestProviderPart(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ provider, part }: SetupPartRef) =>
      unwrap(
        api.POST("/api/v1/administration/providers/{provider}/{part}/test", {
          params: { path: { provider, part } },
        }),
      ),
    onSuccess: () => invalidateProviderSetups(queryClient),
  });
}

export function useRemoveProviderPart(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ provider, part }: SetupPartRef) =>
      unwrap(
        api.DELETE("/api/v1/administration/providers/{provider}/{part}", {
          params: { path: { provider, part } },
        }),
      ),
    onSuccess: () => invalidateProviderSetups(queryClient),
  });
}

export function useConnections(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: connectionsKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/settings/connections")).then(
        (page) => page.items,
      ),
  });
}

/** The Provider Catalog: every provider Pagis connects, with
 *  the form each one needs. The picker draws it and holds no list of
 *  its own. */
export function useConnectionProviders(api: ApiClient) {
  return useQuery({
    queryKey: connectionProvidersKey,
    queryFn: () =>
      unwrap(api.GET("/api/v1/settings/connections/providers")).then(
        (page) => page.items,
      ),
  });
}

export function useCredentials(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: credentialsKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/settings/credentials")).then(
        (page) => page.items,
      ),
  });
}

/** Connect an account (ADR-0012). The fields are the ones the
 *  provider's catalog entry declares. A secret goes one way: the
 *  daemon hands it to the provider or to its encrypted secret store and
 *  stores it nowhere else. */
export function useCreateConnection(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      provider: string;
      alias: string;
      display_name: string;
      fields: Record<string, string>;
    }) => unwrap(api.POST("/api/v1/settings/connections", { body })),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: connectionsKey(workspaceId) }),
  });
}

/** Authorize one connection.
 *
 *  A Google connection answers an `authorization_url` and
 *  returns at once: the caller opens it, the person consents in their own
 *  browser, and the connection reaches `connected` when Google redirects
 *  back to this installation. Every other connection is finished when
 *  this resolves, and `authorization_url` is null. An empty capability
 *  list keeps the read-only profile a new connection starts at. */
export function useAuthorizeConnection(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      connectionId,
      capabilities,
      apiKey,
    }: {
      connectionId: string;
      capabilities?: string[];
      apiKey?: string;
    }) =>
      unwrap(
        api.POST("/api/v1/settings/connections/{connection_id}/authorize", {
          params: { path: { connection_id: connectionId } },
          body: { capabilities: capabilities ?? [], api_key: apiKey },
        }),
      ),
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: connectionsKey(workspaceId) }),
  });
}

export function useDeleteConnection(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (connectionId: string) => {
      const { error, response } = await api.DELETE(
        "/api/v1/settings/connections/{connection_id}",
        { params: { path: { connection_id: connectionId } } },
      );
      if (error !== undefined || !response.ok)
        throw error ?? new Error("delete failed");
    },
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: connectionsKey(workspaceId) }),
  });
}

/** The Workspace's numbers and the carrier they came from. The
 *  `phone_number.*` WS events invalidate it. */
export function usePhoneNumbers(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: phoneNumbersKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/workspace/number")),
  });
}

/** The carrier's own list, read live. Pagis stores no price, so the
 *  search is the one place a price is true (ADR-0018). The query runs
 *  only once the user asks for a place. */
export function useAvailableNumbers(
  api: ApiClient,
  search: { country: string; areaCode: string; locality: string } | null,
) {
  return useQuery({
    queryKey: availableNumbersKey(
      search?.country ?? "",
      search?.areaCode ?? "",
      search?.locality ?? "",
    ),
    enabled: search !== null,
    staleTime: 0,
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/settings/phone-numbers/available", {
          params: {
            query: {
              country: search?.country ?? "",
              area_code: search?.areaCode === "" ? undefined : search?.areaCode,
              locality: search?.locality === "" ? undefined : search?.locality,
            },
          },
        }),
      ).then((page) => page.items),
  });
}

/** Buy the Workspace Number. The purchase intent prevents a second purchase after a restart. */
export function useBuyPhoneNumber(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { e164: string }) =>
      unwrap(api.POST("/api/v1/workspace/number", { body })),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: phoneNumbersKey(workspaceId) }),
  });
}

/** Give the number back to the carrier. The charge stops and the
 *  number never comes back. */
export function useReleasePhoneNumber(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => expectNoContent(api.DELETE("/api/v1/workspace/number")),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: phoneNumbersKey(workspaceId) });
      void queryClient.invalidateQueries({ queryKey: ["available-numbers"] });
    },
  });
}

/** The mailbox section of the Agent creation form and of a later
 *  provision (ADR-0019). The domain is the Connection's, so the form
 *  names the local part alone. */
export interface NewMailboxBody {
  connection_id: string;
  local_part: string;
  outgoing_cap?: number;
  password?: string;
}

/** Every mailbox of the Workspace (ADR-0019). The Connections page
 *  reads it to say which mailboxes sit on a Mailbox Provider. */
export function useMailboxes(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: mailboxesKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/settings/mailboxes")).then((page) => page.items),
  });
}

/** One Agent's mailbox, and what it could have when it holds none. */
export function useAgentMailbox(api: ApiClient, agentId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: agentMailboxKey(workspaceId, agentId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/agents/{agent_id}/mailbox", {
          params: { path: { agent_id: agentId } },
        }),
      ),
  });
}

/** The Mailbox Providers a new Agent could take a mailbox on, and the
 *  free name it would take on each. The Agent does not exist yet, so
 *  the name in the creation form decides the suggestion. */
export function useMailboxOffers(
  api: ApiClient,
  agentName: string,
  enabled = true,
) {
  return useQuery({
    queryKey: mailboxOffersKey(agentName),
    enabled,
    // The name changes with every keystroke; the last answer stays on
    // screen so the section does not flicker away under the user.
    placeholderData: (previous) => previous,
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/settings/mailbox-offers", {
          params: { query: { agent_name: agentName } },
        }),
      ).then((page) => page.offers),
  });
}

/** The live Address Ledger check under the name field (ADR-0019). It
 *  reserves nothing: the ledger still refuses a taken address when the
 *  mailbox is made. */
export function useMailboxName(
  api: ApiClient,
  connectionId: string | null,
  localPart: string,
) {
  return useQuery({
    queryKey: mailboxNameKey(connectionId ?? "", localPart),
    enabled: connectionId !== null && localPart !== "",
    staleTime: 0,
    placeholderData: (previous) => previous,
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/settings/mailbox-offers/name", {
          params: {
            query: { connection_id: connectionId ?? "", local_part: localPart },
          },
        }),
      ),
  });
}

/** Make one Agent Mailbox. The answer arrives once the host has made
 *  it; the login proof follows, so the card starts at `provisioning`
 *  (ADR-0019). */
export function useProvisionMailbox(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ agentId, ...body }: { agentId: string } & NewMailboxBody) =>
      unwrap(
        api.POST("/api/v1/agents/{agent_id}/mailbox", {
          params: { path: { agent_id: agentId } },
          body,
        }),
      ),
    onSuccess: (_mailbox, { agentId }) =>
      invalidateMailboxes(workspaceId, queryClient, agentId),
  });
}

/** Give the mailbox a new password and prove the login again. A host
 *  that mints its own takes no password here. */
export function useResetMailboxPassword(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      agentId,
      password,
    }: {
      agentId: string;
      password?: string;
    }) =>
      unwrap(
        api.POST("/api/v1/agents/{agent_id}/mailbox/reset-password", {
          params: { path: { agent_id: agentId } },
          body: { password },
        }),
      ),
    onSuccess: (_mailbox, { agentId }) =>
      invalidateMailboxes(workspaceId, queryClient, agentId),
  });
}

/** Delete the mailbox. The address is typed back, because the host's
 *  mail goes with it and Pagis keeps no copy (ADR-0019). */
export function useDeleteMailbox(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      agentId,
      confirmAddress,
    }: {
      agentId: string;
      confirmAddress: string;
    }) =>
      unwrap(
        api.DELETE("/api/v1/agents/{agent_id}/mailbox", {
          params: { path: { agent_id: agentId } },
          body: { confirm_address: confirmAddress },
        }),
      ),
    onSuccess: (_deleted, { agentId }) =>
      invalidateMailboxes(workspaceId, queryClient, agentId),
  });
}

/** One mailbox act moves the card, the Workspace list, the Address
 *  Ledger suggestions and the Agent's read-through address. */
function invalidateMailboxes(workspaceId: string, queryClient: QueryClient, agentId: string) {
  void queryClient.invalidateQueries({ queryKey: agentMailboxKey(workspaceId, agentId) });
  void queryClient.invalidateQueries({ queryKey: mailboxesKey(workspaceId) });
  void queryClient.invalidateQueries({ queryKey: ["mailbox-offers"] });
  void queryClient.invalidateQueries({ queryKey: ["mailbox-name"] });
  void queryClient.invalidateQueries({ queryKey: agentsKey(workspaceId) });
}

/** Save a login the user already has. The secret goes one way:
 *  the daemon seals it and no endpoint reads it back. */
export function useAddCredential(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      domain: string;
      username: string;
      login_url: string;
      secret: string;
      totp_seed?: string;
    }) => unwrap(api.POST("/api/v1/settings/credentials", { body })),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: credentialsKey(workspaceId) }),
  });
}

export function useDeleteCredential(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (credentialId: string) => {
      const { error, response } = await api.DELETE(
        "/api/v1/settings/credentials/{credential_id}",
        { params: { path: { credential_id: credentialId } } },
      );
      if (error !== undefined || !response.ok)
        throw error ?? new Error("delete failed");
    },
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: credentialsKey(workspaceId) }),
  });
}

/** One agent computer's state. The `computer.state_changed` WS
 *  events invalidate it. */
export function useComputer(
  api: ApiClient,
  agentId: string,
  { pollMs }: { pollMs?: number } = {},
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: computerKey(workspaceId, agentId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/agents/{agent_id}/computer", {
          params: { path: { agent_id: agentId } },
        }),
      ),
    staleTime: pollMs === undefined ? Infinity : 0,
    // Onboarding runs before the shell opens its event stream, so it
    // asks the daemon again while an image downloads.
    refetchInterval: pollMs ?? false,
  });
}

/**
 * The last screenshot of one agent's screen, as an object URL.
 *
 * The image is a blob the session guards, so it cannot be the
 * `src` of an `img` on its own. The URL lives as long as the blob the
 * cache holds; `computer.state_changed` invalidates the query and the
 * URL is made again. `null` means there is no screen yet.
 */
export function useScreenPreviewUrl(agentId: string): string | null {
  const workspaceId = useCurrentWorkspaceId();
  const preview = useQuery({
    queryKey: screenPreviewKey(workspaceId, agentId),
    queryFn: () => fetchScreenPreview(agentId),
    staleTime: Infinity,
  });
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (preview.data == null) {
      setUrl(null);
      return;
    }
    const objectUrl = URL.createObjectURL(preview.data);
    setUrl(objectUrl);
    return () => URL.revokeObjectURL(objectUrl);
  }, [preview.data]);
  return url;
}

/** Wake one agent's computer. Progress arrives as
 *  `computer.state_changed` events; a 409 names a stale local image. */
export function useWakeComputer(api: ApiClient, agentId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { data, error } = await api.POST(
        "/api/v1/agents/{agent_id}/computer/wake",
        { params: { path: { agent_id: agentId } } },
      );
      if (data === undefined) {
        const detail = (error as { error?: { message?: string } } | undefined)
          ?.error;
        throw new Error(detail?.message ?? "wake failed");
      }
      return data;
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: computerKey(workspaceId, agentId) }),
  });
}

/** Put one agent's computer to sleep now, ahead of the idle
 *  sweep. A 409 says a command runs in it. */
export function useSleepComputer(api: ApiClient, agentId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { data, error } = await api.POST(
        "/api/v1/agents/{agent_id}/computer/sleep",
        { params: { path: { agent_id: agentId } } },
      );
      if (data === undefined) {
        const detail = (error as { error?: { message?: string } } | undefined)
          ?.error;
        throw new Error(detail?.message ?? "the computer did not stop");
      }
      return data;
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: computerKey(workspaceId, agentId) }),
  });
}

/** How many of the given Agents have an awake computer. The
 *  Desk Panel footer counts them. One query per Agent, on the same
 *  keys the tiles read, so a state event moves the count too. */
export function useAwakeCount(
  api: ApiClient,
  agentIds: readonly string[],
): number {
  const workspaceId = useCurrentWorkspaceId();
  const results = useQueries({
    queries: agentIds.map((agentId) => ({
      queryKey: computerKey(workspaceId, agentId),
      queryFn: () =>
        unwrap(
          api.GET("/api/v1/agents/{agent_id}/computer", {
            params: { path: { agent_id: agentId } },
          }),
        ),
      staleTime: Infinity,
    })),
  });
  return results.filter((result) => result.data?.state === "awake").length;
}

/** What the office keeps on disk, for the Desk Panel footer. */
export function useComputerDisk(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: computerDiskKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/computers/disk")),
  });
}

/** Take over the agent's computer: the daemon flips the input
 *  switch to the user and denies the agent's screen leases. */
export function useTakeover(api: ApiClient, agentId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { data, error } = await api.POST(
        "/api/v1/agents/{agent_id}/screen/takeover",
        { params: { path: { agent_id: agentId } } },
      );
      if (data === undefined) {
        const detail = (error as { error?: { message?: string } } | undefined)
          ?.error;
        throw new Error(detail?.message ?? "takeover failed");
      }
      return data;
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: computerKey(workspaceId, agentId) }),
  });
}

/** Hand the computer back to the agent. */
export function useHandback(api: ApiClient, agentId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { data, error } = await api.POST(
        "/api/v1/agents/{agent_id}/screen/handback",
        { params: { path: { agent_id: agentId } } },
      );
      if (data === undefined) {
        const detail = (error as { error?: { message?: string } } | undefined)
          ?.error;
        throw new Error(detail?.message ?? "handback failed");
      }
      return data;
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: computerKey(workspaceId, agentId) }),
  });
}

/** Send into the channel top level, or into one thread (`rootId`). */
export function useSendMessage(
  api: ApiClient,
  channelId: string,
  rootId?: string,
) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  const pending = usePendingSends();
  const scope = threadScope(channelId, rootId);
  return useMutation({
    mutationFn: ({
      pendingId,
      text,
      fileIds,
    }: {
      pendingId: string;
      text: string;
      fileIds?: string[];
    }) =>
      unwrap(
        api.POST("/api/v1/channels/{channel_id}/messages", {
          params: { path: { channel_id: channelId } },
          body: {
            pending_id: pendingId,
            text,
            parent_message_id: rootId,
            file_ids: fileIds ?? [],
          },
        }),
      ),
    onMutate: ({ pendingId, text }) => {
      const existing = usePendingSends
        .getState()
        .byScope[scope]?.some((s) => s.pending_id === pendingId);
      if (existing) {
        pending.markPending(scope, pendingId); // a retry of a failed send
      } else {
        pending.add(scope, {
          pending_id: pendingId,
          text,
          created_at: Date.now(),
          state: "pending",
        });
      }
    },
    onSuccess: (message, { pendingId }) => {
      if (rootId === undefined) {
        insertMessage(workspaceId, queryClient, message);
      } else {
        insertReply(workspaceId, queryClient, rootId, message);
        // The root's rollup changed; recompute the timeline.
        void queryClient.invalidateQueries({
          queryKey: timelineKey(workspaceId, channelId),
        });
      }
      pending.remove(scope, pendingId);
    },
    onError: (_error, { pendingId }) => {
      pending.markFailed(scope, pendingId);
    },
  });
}

// ── Automations (ADR-0006, ADR-0022) ───────────────────────────
//
// Schedules and Event Subscriptions read as workspace lists; each
// detail body adds its paginated occurrence, Incoming Event, and
// Wake-up history. The `schedule.*`, `event_subscription.*`, and
// `wakeup.*` WS events invalidate them, so the destination follows the
// daemon without polling.

/** The rules that wait for a decision, plus every other pending
 *  Request. The needs-you queue reads this one list (ADR-0022: the
 *  queue is a view, never a second source of truth). */
export function usePendingRequests(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: pendingRequestsKey(workspaceId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/requests", {
          params: { query: { state: "pending" } },
        }),
      ).then((page) => page.items),
    staleTime: Infinity,
  });
}

/** The Needs-You Queue, as the daemon derives it (ADR-0022, ADR-0030):
 *  the items in the order of the queue, and their count. Home and the
 *  sidebar count read it. The `needs_you.*` WS events invalidate it. */
export function useNeedsYou(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: needsYouKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/needs-you")),
    staleTime: Infinity,
  });
}

/** Takes one item out of the cached Needs-You Queue, so a dismissed
 *  item leaves Home and the sidebar count before the daemon answers. */
async function dropNeedsYouItem(workspaceId: string, queryClient: QueryClient, itemId: string) {
  await queryClient.cancelQueries({ queryKey: needsYouKey(workspaceId) });
  queryClient.setQueryData<NeedsYouQueue>(needsYouKey(workspaceId), (queue) => {
    if (queue === undefined) return queue;
    const items = queue.items.filter((item) => item.id !== itemId);
    return { items, count: items.length };
  });
}

export function useSchedules(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: schedulesKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/schedules")).then((page) => page.items),
    staleTime: Infinity,
  });
}

export function useSchedule(api: ApiClient, scheduleId: string | null) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: scheduleKey(workspaceId, scheduleId ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/schedules/{schedule_id}", {
          params: { path: { schedule_id: scheduleId! } },
        }),
      ),
    enabled: scheduleId !== null,
    staleTime: Infinity,
  });
}

export function useScheduleOccurrences(
  api: ApiClient,
  scheduleId: string | null,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useInfiniteQuery({
    queryKey: scheduleOccurrencesKey(workspaceId, scheduleId ?? ""),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/schedules/{schedule_id}/occurrences", {
          params: {
            path: { schedule_id: scheduleId! },
            query: { before: pageParam, limit: HISTORY_PAGE },
          },
        }),
      ),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    enabled: scheduleId !== null,
    staleTime: Infinity,
  });
}

export function useScheduleWakeups(api: ApiClient, scheduleId: string | null) {
  const workspaceId = useCurrentWorkspaceId();
  return useInfiniteQuery({
    queryKey: scheduleWakeupsKey(workspaceId, scheduleId ?? ""),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/schedules/{schedule_id}/wakeups", {
          params: {
            path: { schedule_id: scheduleId! },
            query: { before: pageParam, limit: HISTORY_PAGE },
          },
        }),
      ),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    enabled: scheduleId !== null,
    staleTime: Infinity,
  });
}

/** One Schedule control: `pause`, `resume`, `skip_next`, `edit`,
 *  or `archive`. The daemon answers with the row it wrote, and a lost
 *  skip-next race answers 409 with the sentence the caller shows. */
export function useUpdateSchedule(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      scheduleId,
      ...body
    }: {
      scheduleId: string;
      action: "pause" | "resume" | "skip_next" | "edit" | "archive";
      expected_due_at?: number;
      expected_revision?: number;
      name?: string;
      instruction?: string;
      channel_id?: string;
      kind?: string;
      local_time?: string;
      cron_expression?: string;
      interval_minutes?: number;
      timezone?: string;
    }) =>
      unwrap(
        api.POST("/api/v1/schedules/{schedule_id}", {
          params: { path: { schedule_id: scheduleId } },
          body,
        }),
      ),
    onSettled: (_data, _error, { scheduleId }) => {
      void queryClient.invalidateQueries({ queryKey: schedulesKey(workspaceId) });
      void queryClient.invalidateQueries({ queryKey: scheduleKey(workspaceId, scheduleId) });
      void queryClient.invalidateQueries({
        queryKey: scheduleOccurrencesKey(workspaceId, scheduleId),
      });
      void queryClient.invalidateQueries({
        queryKey: scheduleWakeupsKey(workspaceId, scheduleId),
      });
    },
  });
}

export function useSubscriptions(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: subscriptionsKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/event-subscriptions")).then((page) => page.items),
    staleTime: Infinity,
  });
}

/** One Event Subscription with its collector health beside it. */
export function useSubscription(api: ApiClient, subscriptionId: string | null) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: subscriptionKey(workspaceId, subscriptionId ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/event-subscriptions/{subscription_id}", {
          params: { path: { subscription_id: subscriptionId! } },
        }),
      ),
    enabled: subscriptionId !== null,
    staleTime: Infinity,
  });
}

export function useSubscriptionEvents(
  api: ApiClient,
  subscriptionId: string | null,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useInfiniteQuery({
    queryKey: subscriptionEventsKey(workspaceId, subscriptionId ?? ""),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/event-subscriptions/{subscription_id}/events", {
          params: {
            path: { subscription_id: subscriptionId! },
            query: { before: pageParam, limit: HISTORY_PAGE },
          },
        }),
      ),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    enabled: subscriptionId !== null,
    staleTime: Infinity,
  });
}

export function useSubscriptionWakeups(
  api: ApiClient,
  subscriptionId: string | null,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useInfiniteQuery({
    queryKey: subscriptionWakeupsKey(workspaceId, subscriptionId ?? ""),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/event-subscriptions/{subscription_id}/wakeups", {
          params: {
            path: { subscription_id: subscriptionId! },
            query: { before: pageParam, limit: HISTORY_PAGE },
          },
        }),
      ),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    enabled: subscriptionId !== null,
    staleTime: Infinity,
  });
}

/** One Event Subscription control: `pause`, `resume`, `edit`, or
 *  `archive`. There is no skip-next: an Event Subscription has no next
 *  occurrence to skip. */
export function useUpdateSubscription(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      subscriptionId,
      ...body
    }: {
      subscriptionId: string;
      action: "pause" | "resume" | "edit" | "archive";
      name?: string;
      instruction?: string;
      channel_id?: string;
      filter?: unknown;
    }) =>
      unwrap(
        api.POST("/api/v1/event-subscriptions/{subscription_id}", {
          params: { path: { subscription_id: subscriptionId } },
          body,
        }),
      ),
    onSettled: (_data, _error, { subscriptionId }) => {
      void queryClient.invalidateQueries({ queryKey: subscriptionsKey(workspaceId) });
      void queryClient.invalidateQueries({
        queryKey: subscriptionKey(workspaceId, subscriptionId),
      });
      void queryClient.invalidateQueries({
        queryKey: subscriptionEventsKey(workspaceId, subscriptionId),
      });
      void queryClient.invalidateQueries({
        queryKey: subscriptionWakeupsKey(workspaceId, subscriptionId),
      });
    },
  });
}

/** The retention window per file class. */
export function useRetentionPolicies(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: retentionKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/settings/retention")).then((page) => page.items),
  });
}

export function useSetRetentionPolicy(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      kind,
      retainDays,
    }: {
      kind: string;
      retainDays: number | null;
    }) =>
      unwrap(
        api.PUT("/api/v1/settings/retention/{kind}", {
          params: { path: { kind } },
          body: { retain_days: retainDays },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: retentionKey(workspaceId) }),
  });
}

/** One Call record (ADR-0020). It is the one source of truth the
 *  strip, the call inspector and the settled block read; the
 *  `call.transcript` events add the lines that arrive after the read.
 *
 *  A record the bridge has not written yet answers 404, so the read
 *  retries: the strip is minted at the same moment the call starts. */
export function useCall(api: ApiClient, callId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: callKey(workspaceId, callId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/calls/{call_id}", {
          params: { path: { call_id: callId } },
        }),
      ),
    staleTime: Infinity,
  });
}

/** The largest page of the list of Coding Sessions that the daemon
 *  gives. */
const CODING_SESSIONS_PAGE = 100;

/** Every Coding Session of the Workspace, newest first (ADR-0033). The
 *  daemon gives the list in pages before a session, and a full page can
 *  have more after it. The hook reads each page until the last, because
 *  the Coding place sorts every session: an old open session goes above
 *  a new ended one. Each `coding_session.*` frame invalidates it. */
export function useCodingSessions(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const query = useInfiniteQuery({
    queryKey: codingSessionsKey(workspaceId),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/coding-sessions", {
          params: { query: { before: pageParam, limit: CODING_SESSIONS_PAGE } },
        }),
      ),
    getNextPageParam: (page) =>
      page.items.length < CODING_SESSIONS_PAGE ? undefined : page.items.at(-1)?.id,
    staleTime: Infinity,
  });
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);
  return query;
}

/** One Coding Session record (ADR-0033). A `coding_session.*` frame of
 *  the session invalidates it. */
export function useCodingSession(api: ApiClient, sessionId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: codingSessionKey(workspaceId, sessionId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/coding-sessions/{coding_session_id}", {
          params: { path: { coding_session_id: sessionId } },
        }),
      ),
    staleTime: Infinity,
  });
}

/** The Person stops a Coding Session (ADR-0033): the daemon cancels the
 *  turn that runs and closes the session. The record is read again, and
 *  it settles when the daemon writes the terminal state. */
export function useStopCodingSession(api: ApiClient, sessionId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      expectNoContent(
        api.POST("/api/v1/coding-sessions/{coding_session_id}/stop", {
          params: { path: { coding_session_id: sessionId } },
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: codingSessionKey(workspaceId, sessionId) }),
  });
}

/** The largest page of a transcript that the daemon gives. */
const TRANSCRIPT_PAGE = 500;

/** The whole transcript of a Coding Session, oldest row first. The
 *  daemon gives it in pages after a `seq`, and the hook reads each page
 *  until the last, because the session page folds every row. A
 *  `coding_session.*` frame of the session invalidates it, and the
 *  pages are read again. */
export function useCodingSessionEvents(api: ApiClient, sessionId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const query = useInfiniteQuery({
    queryKey: codingSessionEventsKey(workspaceId, sessionId),
    initialPageParam: undefined as number | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/coding-sessions/{coding_session_id}/transcript", {
          params: {
            path: { coding_session_id: sessionId },
            query: { after: pageParam, limit: TRANSCRIPT_PAGE },
          },
        }),
      ),
    getNextPageParam: (page) => page.next_after ?? undefined,
    staleTime: Infinity,
  });
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);
  return query;
}

/** One message, read live through the daemon for the mail inspector
 *  (ADR-0019). Nothing is stored, so the answer is not cached
 *  beyond the open inspector, and a message the host no longer holds
 *  answers 404 rather than an empty body. */
export function useMailMessage(
  api: ApiClient,
  mailbox: string,
  messageId: string,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: mailMessageKey(workspaceId, mailbox, messageId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/mail/{mailbox}/{message_id}", {
          params: { path: { mailbox, message_id: messageId } },
        }),
      ),
    retry: false,
    gcTime: 0,
  });
}

/** The reader dismisses a missed Call from the Needs-You Queue. The
 *  item leaves the cached queue at once, before the daemon answers; the
 *  `needs_you.removed` WS event refreshes the other clients. */
export function useDismissCall(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (callId: string) =>
      expectNoContent(
        api.POST("/api/v1/calls/{call_id}/dismiss", {
          params: { path: { call_id: callId } },
        }),
      ),
    onMutate: (callId) => dropNeedsYouItem(workspaceId, queryClient, `call:${callId}`),
    onSettled: () => queryClient.invalidateQueries({ queryKey: needsYouKey(workspaceId) }),
  });
}

/** The user ends a live call (ADR-0022). */
export function useHangUpCall(api: ApiClient, callId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      expectNoContent(
        api.POST("/api/v1/calls/{call_id}/hangup", {
          params: { path: { call_id: callId } },
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: callKey(workspaceId, callId) }),
  });
}

/** The user drops a live call to Unknown (ADR-0021). It cannot be
 *  undone while the call runs, so the control asks first. */
export function useDropCallTier(api: ApiClient, callId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      unwrap(
        api.POST("/api/v1/calls/{call_id}/tier", {
          params: { path: { call_id: callId } },
          body: { tier: "unknown" },
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: callKey(workspaceId, callId) }),
  });
}

/** The Trust List and the Keypad Code card
 *  (ADR-0021, ADR-0019). No tool writes the list: the user edits it here. */
export function useTrustList(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: trustListKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/settings/trust-list")),
  });
}

export function useAddTrustEntry(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      value: string;
      tier: string;
      label?: string;
      agent_id?: string;
    }) => unwrap(api.POST("/api/v1/settings/trust-list", { body })),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: trustListKey(workspaceId) }),
  });
}

export function useDeleteTrustEntry(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (trustEntryId: string) =>
      expectNoContent(
        api.DELETE("/api/v1/settings/trust-list/{trust_entry_id}", {
          params: { path: { trust_entry_id: trustEntryId } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: trustListKey(workspaceId) }),
  });
}

/** Set the one Keypad Code. It is hashed at the daemon and never read
 *  back: there is no reveal and no export. */
export function useSetKeypadCode(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (code: string) =>
      unwrap(api.PUT("/api/v1/settings/keypad-code", { body: { code } })),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: trustListKey(workspaceId) }),
  });
}

export function useDeleteKeypadCode(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      expectNoContent(api.DELETE("/api/v1/settings/keypad-code")),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: trustListKey(workspaceId) }),
  });
}

/** Clear the failed-attempt count of the Workspace's Keypad Code, and
 *  end its delay (ADR-0021). Only the person knows whether the wrong
 *  codes were theirs, so no tool does this. */
export function useClearKeypadFailures(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      expectNoContent(api.DELETE("/api/v1/settings/keypad-code/failures")),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: trustListKey(workspaceId) }),
        queryClient.invalidateQueries({ queryKey: needsYouKey(workspaceId) }),
      ]),
  });
}

// ── Software (ADR-0022, ADR-0016) ──────────────────────────────
//
// Packages are written by Agents and only read here, so every hook is
// a read. The `software.published` and `contribution.updated` WS
// events invalidate them, so an open page follows the daemon.

/** Every package of the Workspace. */
export function useSoftware(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: softwareKey(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/api/v1/software")).then((page) => page.items),
    staleTime: Infinity,
  });
}

/** One package with its tools, Versions, origin, and Contributions.
 *  The Contributions carry no patch. */
export function useSoftwarePackage(api: ApiClient, name: string | null) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: softwarePackageKey(workspaceId, name ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/software/{name}", {
          params: { path: { name: name! } },
        }),
      ),
    enabled: name !== null,
    staleTime: Infinity,
  });
}

/** One Contribution with its patch. It loads when the reader opens the
 *  patch, because a patch is large and the list never carries it. */
export function useContribution(
  api: ApiClient,
  name: string,
  contributionId: string | null,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: contributionKey(workspaceId, name, contributionId ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/software/{name}/contributions/{contribution_id}", {
          params: { path: { name, contribution_id: contributionId! } },
        }),
      ),
    enabled: contributionId !== null,
    staleTime: Infinity,
  });
}

/** The System Settings (ADR-0024). No staleTime: the read runs a
 *  Docker probe, so opening the tab shows what answers now. */
// ---- What an administrator does to the installation's people ----

/** The roster of the installation, with each person's last sign-in and
 *  monthly spend cap. Administrator only; a member never opens the
 *  section that reads it. */
export function usePeople(api: ApiClient) {
  return useQuery({
    queryKey: peopleKey,
    queryFn: async () =>
      (await unwrap(api.GET("/api/v1/administration/people"))).items,
  });
}

/** The period a spend read answers for: Unix milliseconds, or nothing
 *  for the calendar month the installation is in. */
export interface UsagePeriod {
  from?: number;
  to?: number;
}

/** Spend per Workspace for a period. With no period it is the calendar
 *  month, which is the period the Spend Cap counts. */
export function useInstallationUsage(api: ApiClient, period: UsagePeriod = {}) {
  return useQuery({
    queryKey: installationUsageKey(`${period.from ?? ""}-${period.to ?? ""}`),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/administration/usage", {
          params: { query: { from: period.from, to: period.to } },
        }),
      ),
  });
}

/** How this browser signs in (ADR-0028): `link` where Remote Access
 *  takes no password from its machine, else `password`. The health answer
 *  says it, with no Session. */
export function useSignInMethod(api: ApiClient) {
  return useQuery({
    queryKey: healthKey,
    queryFn: async () => (await unwrap(api.GET("/api/v1/health"))).sign_in,
    staleTime: Infinity,
  });
}

/** What the server's own first run still needs. It answers
 *  while no administrator can sign in, and `410 Gone` from the first
 *  password onwards. The `410` is an answer, not a failure: the data is
 *  `null` then, so a read that answered the setup before and `410` after
 *  replaces the setup rather than keeping it beside an error. */
export function useSetupState(api: ApiClient) {
  return useQuery({
    queryKey: setupKey,
    queryFn: async () => {
      const { data, error } = await api.GET("/api/v1/setup");
      if (data !== undefined) return data;
      if (errorCode(error) === "setup_complete") return null;
      throw error ?? new Error("request failed");
    },
    retry: false,
    staleTime: Infinity,
  });
}

/** Make the installation's first administrator and keep the provider
 *  keys. The answer signs the new administrator in. */
export function useCompleteSetup(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      email: string;
      password: string;
      name?: string;
      provider_keys?: Record<string, string>;
    }) =>
      unwrap(
        api.POST("/api/v1/setup", {
          body: { ...body, timezone: deviceTimezone() },
        }),
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: setupKey });
      void queryClient.invalidateQueries({ queryKey: userKey });
    },
  });
}

/** Who is signed in, from which kind of client, and since when.
 *  Administrator only, and the Administration Interface is where it is
 *  read. No staleTime: the page says who is signed in now. */
export function useLiveSessions(api: ApiClient) {
  return useQuery({
    queryKey: liveSessionsKey,
    queryFn: async () =>
      (await unwrap(api.GET("/api/v1/administration/sessions"))).items,
  });
}

/** The containers, volumes, disk and awake Computers of each person. */
export function useInstallationResources(api: ApiClient) {
  return useQuery({
    queryKey: resourcesKey,
    queryFn: () => unwrap(api.GET("/api/v1/administration/resources")),
  });
}

/** What the daemon runs, what it keeps its records in, where it reaches
 *  Docker, and how much work waits. */
export function useInstallationHealth(api: ApiClient) {
  return useQuery({
    queryKey: installationHealthKey,
    queryFn: () => unwrap(api.GET("/api/v1/administration/health")),
  });
}

/** Give a person the address and the password they sign in with.
 *  The seeded person of a local installation holds neither until they
 *  want a browser to sign in without the client. */
export function useSetSignIn(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      userId,
      email,
      password,
    }: {
      userId: string;
      email: string;
      password: string;
    }) =>
      unwrap(
        api.PUT("/api/v1/administration/people/{user_id}/sign-in", {
          params: { path: { user_id: userId } },
          body: { email, password },
        }),
      ),
    onSuccess: () => invalidatePeople(queryClient),
  });
}

/** What the current Workspace spent this month, and its Spend Cap. Every person reads their own. */
export function useMyUsage(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: myUsageKey(workspaceId),
    queryFn: () => unwrap(api.GET("/api/v1/usage")),
  });
}

/** Every read of the administration surfaces, after a write. */
function invalidatePeople(queryClient: QueryClient) {
  void queryClient.invalidateQueries({ queryKey: peopleKey });
  void queryClient.invalidateQueries({ queryKey: ["administration", "usage"] });
  void queryClient.invalidateQueries({ queryKey: liveSessionsKey });
}

/** Create an account. The answer holds the person's invite: a Sign-In
 *  Link good for seven days and one use. A first password is optional. */
export function useCreateAccount(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { email: string; name: string; password?: string; mailbox_address?: string }) =>
      unwrap(api.POST("/api/v1/administration/people", { body })),
    onSuccess: () => invalidatePeople(queryClient),
  });
}

/** Make a new invite for a person whose invite expired, or who has no
 *  Session left. */
export function useMakeInvite(api: ApiClient) {
  return useMutation({
    mutationFn: (userId: string) =>
      unwrap(
        api.POST("/api/v1/administration/people/{user_id}/sign-in-links", {
          params: { path: { user_id: userId } },
        }),
      ),
  });
}

export function useSetAccountEnabled(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ userId, enabled }: { userId: string; enabled: boolean }) =>
      unwrap(
        api.POST(
          enabled
            ? "/api/v1/administration/people/{user_id}/enable"
            : "/api/v1/administration/people/{user_id}/disable",
          { params: { path: { user_id: userId } } },
        ),
      ),
    onSuccess: () => invalidatePeople(queryClient),
  });
}

export function useResetAccountPassword(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ userId, password }: { userId: string; password: string }) =>
      unwrap(
        api.POST("/api/v1/administration/people/{user_id}/password", {
          params: { path: { user_id: userId } },
          body: { password },
        }),
      ),
    onSuccess: () => invalidatePeople(queryClient),
  });
}

/** Change a Person's role in the Org. */
export function useSetPersonRole(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ userId, role }: { userId: string; role: "administrator" | "member" }) =>
      unwrap(api.PUT("/api/v1/administration/people/{user_id}/role", {
        params: { path: { user_id: userId } },
        body: { role },
      })),
    onSuccess: () => {
      invalidatePeople(queryClient);
      void queryClient.invalidateQueries({ queryKey: userKey });
    },
  });
}

export const workspacesKey = ["administration", "workspaces"] as const;

export function useAdministrationWorkspaces(api: ApiClient) {
  return useQuery({ queryKey: workspacesKey, queryFn: async () =>
    (await unwrap(api.GET("/api/v1/administration/workspaces"))).items });
}

export function useSetSpendCap(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ workspaceId, capUsd }: { workspaceId: string; capUsd: number | null }) =>
      unwrap(api.PUT("/api/v1/administration/workspaces/{workspace_id}/spend-cap", {
        params: { path: { workspace_id: workspaceId } },
        body: { monthly_spend_cap_usd: capUsd },
      })),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: workspacesKey });
      void queryClient.invalidateQueries({ queryKey: ["administration", "usage"] });
      void queryClient.invalidateQueries({ queryKey: ["workspace"] });
    },
  });
}

export function useSystemSettings(api: ApiClient) {
  return useQuery({
    queryKey: systemSettingsKey,
    queryFn: () => unwrap(api.GET("/api/v1/settings/system")),
  });
}

export function useAdministrationSections(api: ApiClient) {
  return useQuery({
    queryKey: ["administration", "sections"],
    queryFn: () => unwrap(api.GET("/api/v1/administration/sections")),
  });
}

export function useSaveSystemSettings(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: SystemSettingsBody) =>
      unwrap(api.PUT("/api/v1/settings/system", { body })),
    onSuccess: (saved) => {
      queryClient.setQueryData(systemSettingsKey, saved.settings);
    },
  });
}

/** How often the switch reads Remote Access while a turn-on waits for
 *  Tailscale. */
const TURN_ON_POLL_MS = 1_000;

/** Remote Access and the Tailscale of the machine (ADR-0028). While a
 *  turn-on waits, the read comes again each second, so the switch shows
 *  the page that Tailscale names and the end of the turn-on. */
export function useRemoteAccess(api: ApiClient) {
  return useQuery({
    queryKey: remoteAccessKey,
    queryFn: () => unwrap(api.GET("/api/v1/settings/system/remote-access")),
    refetchInterval: (query) =>
      query.state.data?.turning_on ? TURN_ON_POLL_MS : false,
  });
}

/** Turn Remote Access on. The daemon turns on Tailscale Funnel in the
 *  background and answers at once; the read follows the turn-on. */
export function useTurnOnRemoteAccess(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.PUT("/api/v1/settings/system/remote-access")),
    onSuccess: (remoteAccess) => {
      queryClient.setQueryData(remoteAccessKey, remoteAccess);
    },
  });
}

/** Turn Remote Access off, or stop a turn-on that waits. The daemon
 *  removes the Funnel and clears the settings; a restart puts it in
 *  effect. */
export function useTurnOffRemoteAccess(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.DELETE("/api/v1/settings/system/remote-access")),
    onSuccess: (remoteAccess) => {
      queryClient.setQueryData(remoteAccessKey, remoteAccess);
    },
  });
}

/** Turn the anonymous analytics on or off (ADR-0026). The daemon reads
 *  the setting at each check, so no restart is needed. */
export function useSetAnalytics(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (enabled: boolean) =>
      unwrap(api.PUT("/api/v1/settings/system/analytics", { body: { enabled } })),
    onSuccess: (saved) => {
      queryClient.setQueryData(systemSettingsKey, saved.settings);
    },
  });
}

/** Turn Model Request Capture on or off and set its retention
 *  (ADR-0031). The agent loop reads the live setting, so no restart is
 *  needed. Turning it off deletes every capture. */
export function useSetModelRequestCapture(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { enabled: boolean; retention_days: number }) =>
      unwrap(api.PUT("/api/v1/settings/system/model-request-capture", { body })),
    onSuccess: (saved) => {
      queryClient.setQueryData(systemSettingsKey, saved.settings);
      void queryClient.invalidateQueries({ queryKey: userKey });
    },
  });
}

/** Turn the Home Exit off or on for every person of a server
 *  (ADR-0029). The awake Computers switch at once, with no restart. */
export function useSetHomeExitSetting(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (enabled: boolean) =>
      unwrap(api.PUT("/api/v1/settings/system/home-exit", { body: { enabled } })),
    onSuccess: (saved) => {
      queryClient.setQueryData(systemSettingsKey, saved.settings);
    },
  });
}

/** "Probe again": the daemon pings every candidate now. */
export function useProbeDocker(api: ApiClient) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.POST("/api/v1/settings/system/docker/probe")),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: systemSettingsKey });
    },
  });
}

/** "Restart now": the daemon exits with the reserved code, and the
 *  Client App starts it again. */
export function useRestartDaemon(api: ApiClient) {
  return useMutation({
    mutationFn: () => unwrap(api.POST("/api/v1/system/restart")),
  });
}

// ── Plugins (ADR-0017) ─────────────────────
//
// Installing, binding, updating and granting a Plugin is the user's
// own act, so every write here is a desk write. The `plugin.*` WS
// events invalidate the reads, so an open page follows the daemon.

/** Every installed Plugin of the Workspace. */
export function usePlugins(api: ApiClient) {
  return useQuery({
    queryKey: pluginsKey,
    queryFn: () =>
      unwrap(api.GET("/api/v1/plugins")).then((page) => page.items),
  });
}

/** One Widget page (ADR-0016). A Version never changes, so the
 *  page is read once and kept. */
export function useWidgetPage(
  packageName: string,
  version: string,
  widget: string,
  enabled: boolean,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: widgetPageKey(workspaceId, packageName, version, widget),
    queryFn: () => fetchWidgetPage(packageName, version, widget),
    enabled,
    retry: false,
    staleTime: Infinity,
  });
}

/** One Plugin with its servers, fields, bindings, tools and Skills.
 *  It is the install card and the plugin page at once. */
export function usePlugin(api: ApiClient, pluginId: string | null) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: pluginKey(workspaceId, pluginId ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/plugins/{plugin_id}", {
          params: { path: { plugin_id: pluginId! } },
        }),
      ),
    enabled: pluginId !== null,
    staleTime: Infinity,
  });
}

/** The end of one Plugin's server log (ADR-0017). No staleTime: the
 *  log grows while a server runs, so a read shows what is there now. */
export function usePluginLog(api: ApiClient, pluginId: string | null) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: pluginLogKey(workspaceId, pluginId ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/plugins/{plugin_id}/log", {
          params: { path: { plugin_id: pluginId! } },
        }),
      ).then((log) => log.text),
    enabled: pluginId !== null,
  });
}

/** Install from a git URL or from one uploaded tar. The answer is the
 *  install card: what the package declares, and what it binds. */
export function useInstallPlugin(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (source: PluginSourceRequest) =>
      unwrap(api.POST("/api/v1/plugins", { body: { source } })),
    onSuccess: (plugin) => {
      queryClient.setQueryData(pluginKey(workspaceId, plugin.id), plugin);
      void queryClient.invalidateQueries({ queryKey: pluginsKey });
    },
  });
}

/** Bind one declared field. A secret goes one way: the daemon keeps
 *  it and the desk never reads it back (ADR-0017). The Plugin enables
 *  itself when its last required Binding arrives. */
export function useBindPluginField(api: ApiClient, pluginId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      field,
      value,
    }: {
      field: string;
      value: BindingValueRequest;
    }) =>
      unwrap(
        api.PUT("/api/v1/plugins/{plugin_id}/bindings/{field}", {
          params: { path: { plugin_id: pluginId, field } },
          body: value,
        }),
      ),
    onSuccess: (plugin) => {
      queryClient.setQueryData(pluginKey(workspaceId, pluginId), plugin);
      void queryClient.invalidateQueries({ queryKey: pluginsKey });
    },
  });
}

/** Read the source again and install the next state. The answer names
 *  the files that changed and carries the new tools (ADR-0017). */
export function useUpdatePlugin(api: ApiClient, pluginId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (fileId?: string) =>
      unwrap(
        api.POST("/api/v1/plugins/{plugin_id}/update", {
          params: { path: { plugin_id: pluginId } },
          body: { file_id: fileId },
        }),
      ),
    onSuccess: (plugin) => {
      queryClient.setQueryData(pluginKey(workspaceId, pluginId), plugin);
      void queryClient.invalidateQueries({ queryKey: pluginsKey });
    },
  });
}

/** Start every server of a Plugin that failed (ADR-0017). */
export function useStartPlugin(api: ApiClient, pluginId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      unwrap(
        api.POST("/api/v1/plugins/{plugin_id}/start", {
          params: { path: { plugin_id: pluginId } },
        }),
      ),
    onSuccess: (plugin) => {
      queryClient.setQueryData(pluginKey(workspaceId, pluginId), plugin);
      void queryClient.invalidateQueries({ queryKey: pluginsKey });
      void queryClient.invalidateQueries({ queryKey: pluginLogKey(workspaceId, pluginId) });
    },
  });
}

/** Uninstall: the servers stop, the Grants go, and the directories go
 *  with them. */
export function useUninstallPlugin(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (pluginId: string) => {
      const { error, response } = await api.DELETE(
        "/api/v1/plugins/{plugin_id}",
        { params: { path: { plugin_id: pluginId } } },
      );
      if (error !== undefined || !response.ok) {
        throw error ?? new Error("uninstall failed");
      }
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: pluginsKey });
      void queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) });
    },
  });
}

/** Give one Agent the Plugin Grant: all of the Plugin, or none of it. */
export function useGrantPlugin(api: ApiClient, pluginId: string) {
  const workspaceId = useCurrentWorkspaceId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (agentId: string) =>
      unwrap(
        api.POST("/api/v1/plugins/{plugin_id}/grants", {
          params: { path: { plugin_id: pluginId } },
          body: { agent_id: agentId },
        }),
      ),
    onSettled: () => queryClient.invalidateQueries({ queryKey: grantsKey(workspaceId) }),
  });
}
/** The data half of one live Widget view. It is gone once the Run that
 *  minted it ended, and the block then renders the page without data. */
export function useWidgetView(
  api: ApiClient,
  toolCallId: string,
  enabled: boolean,
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: widgetViewKey(workspaceId, toolCallId),
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/widgets/{tool_call_id}/view", {
          params: { path: { tool_call_id: toolCallId } },
        }),
      ),
    enabled,
    retry: false,
    staleTime: Infinity,
  });
}

export const workspaceMailKey = (workspaceId: string) =>
  ["workspace", workspaceId, "mail"] as const;
export const workspaceMailMessagesKey = (workspaceId: string) =>
  [...workspaceMailKey(workspaceId), "messages"] as const;

export function useWorkspaceMailbox(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...workspaceMailKey(workspaceId), "mailbox"],
    queryFn: () => unwrap(api.GET("/api/v1/workspace/mailbox")),
  });
}

export function useWorkspaceMailFolders(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...workspaceMailKey(workspaceId), "folders"],
    queryFn: () => unwrap(api.GET("/api/v1/workspace/mail/folders")),
  });
}

export function useWorkspaceMailMessages(
  api: ApiClient,
  query: { folder: string; q: string; cursor?: string; reason?: string; view?: "all" | "needs_you" },
) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...workspaceMailMessagesKey(workspaceId), query],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/workspace/mail/messages", {
          params: { query },
        }),
      ),
  });
}

/** Opening mail marks it as seen. The place refreshes the list on return. */
export function useWorkspaceMailMessage(api: ApiClient, messageId: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...workspaceMailKey(workspaceId), "message", messageId],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/workspace/mail/messages/{message_id}", {
          params: { path: { message_id: messageId } },
        }),
      ),
    retry: false,
    gcTime: 0,
  });
}

export function useWorkspaceMailThread(api: ApiClient, threadId?: string) {
  const workspaceId = useCurrentWorkspaceId();
  return useQuery({
    queryKey: [...workspaceMailKey(workspaceId), "thread", threadId],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/workspace/mail/threads/{thread_id}", {
          params: { path: { thread_id: threadId! } },
        }),
      ),
    enabled: threadId !== undefined,
    retry: false,
    gcTime: 0,
  });
}

export type MailAction = 'reply' | 'reply-all' | 'forward'
export type MailSource = { id: string; action: MailAction }

export function useWorkspaceMailComposer(api: ApiClient, source?: MailSource) {
  const workspaceId = useCurrentWorkspaceId()
  return useQuery({
    queryKey: [...workspaceMailKey(workspaceId), 'composer', source],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/workspace/mail/messages/{message_id}/composer', {
          params: {
            path: { message_id: source!.id },
            query: { action: source!.action },
          },
        }),
      ),
    enabled: source !== undefined,
    retry: false,
  })
}

export function useSendWorkspaceMail(api: ApiClient, source?: MailSource) {
  const workspaceId = useCurrentWorkspaceId()
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: components['schemas']['SendMailRequest']) => {
      if (source) {
        const path = {
          reply: '/api/v1/workspace/mail/messages/{message_id}/reply',
          'reply-all': '/api/v1/workspace/mail/messages/{message_id}/reply-all',
          forward: '/api/v1/workspace/mail/messages/{message_id}/forward',
        } as const
        return unwrap(
          api.POST(path[source.action], {
            params: { path: { message_id: source.id } },
            body,
          }),
        )
      }
      return unwrap(api.POST('/api/v1/workspace/mail/messages', { body }))
    },
    onSuccess: () =>
      client.invalidateQueries({ queryKey: workspaceMailKey(workspaceId) }),
  })
}

export function useMailFolderMutation(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId()
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (request: {
      action: 'create' | 'rename' | 'delete'
      name: string
      newName?: string
    }) => {
      let result
      if (request.action === 'create') {
        result = await api.POST('/api/v1/workspace/mail/folders', {
          body: { name: request.name },
        })
      } else if (request.action === 'rename') {
        result = await api.PATCH('/api/v1/workspace/mail/folders/{name}', {
          params: { path: { name: request.name } },
          body: { name: request.newName! },
        })
      } else {
        result = await api.DELETE('/api/v1/workspace/mail/folders/{name}', {
          params: { path: { name: request.name } },
        })
      }
      if (result.error) throw result.error
    },
    onSuccess: () =>
      client.invalidateQueries({ queryKey: workspaceMailKey(workspaceId) }),
  })
}

export function useOrganizeWorkspaceMail(api: ApiClient, id: string) {
  const workspaceId = useCurrentWorkspaceId()
  const client = useQueryClient()
  return useMutation({
    mutationFn: async ({
      action,
      name,
    }: {
      action: 'move' | 'label' | 'unlabel'
      name: string
    } | {
      action: 'delete'
      name?: never
    }) => {
      let result
      if (action === 'delete') {
        result = await api.DELETE(
          '/api/v1/workspace/mail/messages/{message_id}',
          { params: { path: { message_id: id } } },
        )
      } else if (action === 'unlabel') {
        result = await api.DELETE(
          '/api/v1/workspace/mail/messages/{message_id}/labels/{label}',
          { params: { path: { message_id: id, label: name } } },
        )
      } else {
        const path =
          action === 'move'
            ? '/api/v1/workspace/mail/messages/{message_id}/move'
            : '/api/v1/workspace/mail/messages/{message_id}/labels'
        result = await api.POST(path, {
          params: { path: { message_id: id } },
          body: { name },
        })
      }
      if (result.error) throw result.error
    },
    onSuccess: () =>
      client.invalidateQueries({ queryKey: workspaceMailKey(workspaceId) }),
  })
}

export function useSetDelegateEverything(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId()
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (enabled: boolean) => unwrap(api.PUT('/api/v1/workspace/delegate-everything', { body: { enabled } })),
    onSuccess: saved => queryClient.setQueryData(['workspace', workspaceId, 'details'], saved),
  })
}
