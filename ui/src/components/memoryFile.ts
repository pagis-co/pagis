// Where one memory file of a commit is read: the `scope` and
// `path` query values of `/api/v1/memory/file`.

/** The read target of one commit file (`shared/…`, or the commit
 *  agent's `private/…`). Null when the file has no readable scope. */
export function memoryFileTarget(
  agentId: string | null | undefined,
  file: string,
): { scope: string; path: string } | null {
  if (file.startsWith('shared/')) {
    return { scope: 'shared', path: file.slice('shared/'.length) }
  }
  if (file.startsWith('private/') && agentId != null) {
    return { scope: `agent:${agentId}`, path: file.slice('private/'.length) }
  }
  return null
}
