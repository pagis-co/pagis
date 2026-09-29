import { execFile } from 'node:child_process'

/**
 * Read the port from the daemon's taken-port message (ADR-0025). The
 * daemon exits with it when it cannot bind, because that is the one
 * failure it cannot report through the Product App. The daemon names
 * only the port; the client names the process that holds it.
 */
export function parseTakenPort(output: string): number | null {
  // The administration port has a message of its own, and a setting and not
  // this page moves it.
  const found = /(?<!administration )\bport (\d+) is already in use\. Stop the process that holds it/.exec(output)
  return found ? Number(found[1]) : null
}

/**
 * Read the port from the daemon's message for a taken Administration
 * Port. The config file and not the taken-port page moves that port,
 * so the client names the process that holds it and proposes no port.
 */
export function parseTakenAdministrationPort(output: string): number | null {
  const found = /\badministration port (\d+) is already in use\. Stop the process that holds it/.exec(output)
  return found ? Number(found[1]) : null
}

/** Run a program and give its standard output, or null when it fails. */
export type Probe = (program: string, args: string[]) => Promise<string | null>

const probe: Probe = (program, args) => new Promise((resolve) => {
  execFile(program, args, { encoding: 'utf8', timeout: 5000 }, (error, stdout) => {
    resolve(error ? null : stdout)
  })
})

/**
 * The process that listens on `port`, as a person reads it: its name
 * and its process id. macOS asks `lsof`. Linux asks `ss` from iproute2,
 * which every desktop distribution installs; it names the processes of
 * this account only. Where neither answers, the page names the port
 * alone.
 */
export async function portHolder(
  port: number,
  platform: string = process.platform,
  run: Probe = probe,
): Promise<string | null> {
  if (platform === 'linux') {
    const output = await run('ss', ['-Hltnp', `sport = :${port}`])
    const found = output && /users:\(\("((?:[^"\\]|\\.)+)",pid=(\d+)/.exec(output)
    return found ? `${found[1]} (pid ${found[2]})` : null
  }
  const output = await run('/usr/sbin/lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-Fpc'])
  if (!output) return null
  const pid = /^p(\d+)$/m.exec(output)
  const command = /^c(.+)$/m.exec(output)
  return pid && command ? `${command[1]} (pid ${pid[1]})` : null
}
