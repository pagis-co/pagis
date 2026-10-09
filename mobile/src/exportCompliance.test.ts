import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * The app uses only the encryption of iOS: HTTPS, and the decryption of
 * a Web Push with CryptoKit. That encryption is exempt. When the app
 * declares this in its Info.plist, App Store Connect does not ask the
 * export compliance question for each build.
 */

const infoPlist = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'ios/App/App/Info.plist')

/** The value element that follows `<key>name</key>` in the top-level dict. */
function topLevelValue(path: string, name: string): Element | undefined {
  const doc = new DOMParser().parseFromString(readFileSync(path, 'utf8'), 'application/xml')
  const children = [...doc.documentElement.firstElementChild!.children]
  const at = children.findIndex((node) => node.tagName === 'key' && node.textContent === name)
  return at === -1 ? undefined : children[at + 1]
}

describe('the export compliance of the iOS app', () => {
  it('declares that the app uses only exempt encryption', () => {
    expect(topLevelValue(infoPlist, 'ITSAppUsesNonExemptEncryption')?.tagName).toBe('false')
  })
})
