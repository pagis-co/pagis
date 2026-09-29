import { spawn } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { describe, expect, it } from 'vitest'

import { sleep } from './health'
import { classifyLsof, executableInUse, procHolds } from './runtimeUse'

describe('installed runtime ownership check', () => {
  it('treats lsof diagnostics as inspection uncertainty', () => {
    const noMatch = Object.assign(new Error('no matches'), { code: 1 })
    expect(classifyLsof(noMatch, '', '')).toBe(false)
    expect(() => classifyLsof(noMatch, '', 'permission denied')).toThrow(/could not confirm/)
    expect(() => classifyLsof(null, '', 'inspection warning')).toThrow(/could not confirm/)
  })
  it('finds an unresponsive external process with the exact executable open', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-runtime-use-'))
    const binary = path.join(root, 'pagis')
    fs.writeFileSync(binary, 'fixture')
    const child = spawn(process.execPath, ['-e', `require('fs').openSync(${JSON.stringify(binary)}, 'r'); setInterval(() => {}, 1000)`])
    try {
      for (let tries = 0; tries < 100 && !(await executableInUse(binary)); tries += 1) await sleep(10)
      expect(await executableInUse(binary)).toBe(true)
    } finally {
      child.kill('SIGKILL')
      fs.rmSync(root, { recursive: true, force: true })
    }
  })

  it('reads the executable, the open descriptors and the mappings of each process on Linux', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-proc-'))
    try {
      const binary = path.join(root, 'pagis')
      fs.writeFileSync(binary, 'fixture')
      const proc = path.join(root, 'proc')
      const processDir = (pid: string) => {
        fs.mkdirSync(path.join(proc, pid, 'fd'), { recursive: true })
        return path.join(proc, pid)
      }
      fs.symlinkSync('/usr/bin/other', path.join(processDir('10'), 'exe'))
      fs.mkdirSync(path.join(proc, 'self'))
      expect(procHolds(binary, proc)).toBe(false)

      fs.symlinkSync(binary, path.join(processDir('11'), 'exe'))
      expect(procHolds(binary, proc)).toBe(true)
      fs.rmSync(path.join(proc, '11'), { recursive: true })

      fs.symlinkSync(binary, path.join(processDir('12'), 'fd', '3'))
      expect(procHolds(binary, proc)).toBe(true)
      fs.rmSync(path.join(proc, '12'), { recursive: true })

      fs.writeFileSync(path.join(processDir('13'), 'maps'), `7f00-7f01 r-xp 00000000 08:01 42 ${binary}\n`)
      expect(procHolds(binary, proc)).toBe(true)
    } finally {
      fs.rmSync(root, { recursive: true, force: true })
    }
  })
})
