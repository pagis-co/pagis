import { sleep } from './health'

/** How long a daemon has to stop on SIGINT before it is killed. */
export const STOP_DEADLINE_MS = 5000

export function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

/**
 * Stop a daemon the way Ollama's app does: SIGINT, then a kill when
 * the deadline passes. Returns true when the process is gone.
 */
export async function stopProcess(
  pid: number,
  deadlineMs = STOP_DEADLINE_MS,
): Promise<boolean> {
  if (!isAlive(pid)) {
    return true
  }
  send(pid, 'SIGINT')
  const deadline = Date.now() + deadlineMs
  while (Date.now() < deadline) {
    if (!isAlive(pid)) {
      return true
    }
    await sleep(50)
  }
  send(pid, 'SIGKILL')
  await sleep(100)
  return !isAlive(pid)
}

function send(pid: number, signal: NodeJS.Signals): void {
  try {
    process.kill(pid, signal)
  } catch {
    // The process ended between the check and the signal.
  }
}
