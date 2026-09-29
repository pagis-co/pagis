/** What `GET /api/v1/health` answers. The endpoint needs no Session. */
export interface DaemonHealth {
  status: string
  version: string
}

/**
 * Ask the server at an origin whether it is up. Returns null when
 * nothing answers, or when what answers is not a Pagis server. The
 * origin is the loopback one of a server this client started, or the one
 * the person named for a server it did not start.
 */
export async function probeHealth(
  origin: string,
  timeoutMs = 1000,
  request: typeof fetch = fetch,
  signal?: AbortSignal,
): Promise<DaemonHealth | null> {
  const timeout = AbortSignal.timeout(timeoutMs)
  try {
    const response = await request(new URL('/api/v1/health', origin), {
      redirect: 'error',
      signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
    })
    if (!response.ok) {
      return null
    }
    const body = (await response.json()) as Partial<DaemonHealth>
    if (body.status !== 'ok' || typeof body.version !== 'string') {
      return null
    }
    return { status: body.status, version: body.version }
  } catch {
    return null
  }
}

/** Poll the health endpoint until the server answers or time runs out. */
export async function waitForHealth(
  origin: string,
  options: { timeoutMs: number; intervalMs?: number; cancelled?: () => boolean },
): Promise<DaemonHealth | null> {
  const interval = options.intervalMs ?? 100
  const deadline = Date.now() + options.timeoutMs
  while (Date.now() < deadline) {
    if (options.cancelled?.()) {
      return null
    }
    const health = await probeHealth(origin)
    if (health) {
      return health
    }
    await sleep(interval)
  }
  return null
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
