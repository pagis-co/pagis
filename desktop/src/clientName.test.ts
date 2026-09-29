// The name of the Client App. Electron takes `productName` from
// package.json for `app.name`, so the name decides the directory of
// `userData` and the "About" and "Hide" items of the macOS app menu.
// electron-builder takes the same field for the application and its
// packages, so the name has one source.

import * as fs from 'node:fs'
import * as path from 'node:path'

import { describe, expect, it } from 'vitest'

const desktop = path.join(__dirname, '..')

describe('the name of the Client App', () => {
  it('is Pagis, from package.json alone', () => {
    const manifest = JSON.parse(fs.readFileSync(path.join(desktop, 'package.json'), 'utf8')) as Record<string, unknown>
    const builder = fs.readFileSync(path.join(desktop, 'electron-builder.yml'), 'utf8')

    expect(manifest.productName).toBe('Pagis')
    expect(builder).not.toMatch(/^productName:/m)
  })
})
