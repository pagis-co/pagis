import * as fs from 'node:fs'
import * as path from 'node:path'

/**
 * The PID file records the daemon the shell spawned, so that a shell
 * which was killed leaves nothing behind (ADR-0025). It sits beside
 * the shell's own state, in the application data directory: Application
 * Support on macOS, and `~/.config/Pagis` on Linux.
 */
export class PidFile {
  constructor(private readonly file: string) {}

  static inside(directory: string): PidFile {
    return new PidFile(path.join(directory, 'pagis.pid'))
  }

  get path(): string {
    return this.file
  }

  write(pid: number): void {
    fs.mkdirSync(path.dirname(this.file), { recursive: true })
    fs.writeFileSync(this.file, `${pid}\n`)
  }

  read(): number | null {
    try {
      const pid = Number(fs.readFileSync(this.file, 'utf8').trim())
      return Number.isInteger(pid) && pid > 0 ? pid : null
    } catch {
      return null
    }
  }

  clear(): void {
    try {
      fs.rmSync(this.file)
    } catch {
      // Already gone.
    }
  }

}
