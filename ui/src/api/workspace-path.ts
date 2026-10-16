/** The active Shared Workspace prefix for Product App requests. */
let sharedWorkspaceId: string | null = null

export function selectWorkspace(id: string | null) {
  sharedWorkspaceId = id
}

const workspaceResources = new Set([
  'agents', 'calls', 'channels', 'coding-sessions', 'computers',
  'connections', 'event-subscriptions', 'files', 'grants', 'hosts',
  'knowledge', 'mail', 'memory', 'needs-you', 'plugins', 'requests',
  'runs', 'schedules', 'settings', 'software', 'usage', 'widgets',
  'workspace', 'ws',
])

export function workspacePath(path: string, id = sharedWorkspaceId, method = 'GET'): string {
  if (id === null || !path.startsWith('/api/v1/')) return path
  const relative = path.slice('/api/v1/'.length).split('?')[0]
  const resource = relative.split('/')[0]
  if (!workspaceResources.has(resource)) return path
  if (/^settings\/(onboarding|system|sessions|sign-in-links|models|mailbox-offers|phone-numbers|connections\/providers)(\/|$)/.test(relative)) return path
  if (resource === 'plugins') {
    if (relative === 'plugins') return path
    if (/^plugins\/[^/]+$/.test(relative) && method.toLowerCase() !== 'get') return path
    if (/^plugins\/[^/]+\/(update|bindings|start)(\/|$)/.test(relative)) return path
  }
  if (/^connections\/google\/(start|callback)$/.test(relative)) return path
  if (/^widgets\/[^/]+\/[^/]+\/[^/]+\/[^/]+\/sandbox$/.test(relative)) return path
  return `/api/v1/workspaces/${encodeURIComponent(id)}${path.slice('/api/v1'.length)}`
}
