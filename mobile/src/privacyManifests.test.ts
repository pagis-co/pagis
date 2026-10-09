import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * App Store Connect refuses an upload whose code calls a required-reason
 * API with no reason in a privacy manifest. Each bundle that holds code
 * holds its own manifest: the app and the Notification Service
 * Extension.
 */

const mobile = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const ios = resolve(mobile, 'ios/App')

/** The bundles of the iOS project that hold code, with their manifests. */
const bundles = [
  { name: 'App', manifest: 'App/PrivacyInfo.xcprivacy' },
  { name: 'PagisNotificationService', manifest: 'PagisNotificationService/PrivacyInfo.xcprivacy' },
]

/** The symbols of each required-reason API category that Apple lists. */
const requiredReasonApis: Record<string, RegExp> = {
  NSPrivacyAccessedAPICategoryUserDefaults: /\bUserDefaults\b|\bNSUserDefaults\b/,
  NSPrivacyAccessedAPICategoryFileTimestamp:
    /\b(creationDate|modificationDate|contentModificationDate|fileModificationDate|attributesOfItem|getattrlist|fstat|lstat)\b/,
  NSPrivacyAccessedAPICategorySystemBootTime: /\b(systemUptime|mach_absolute_time)\b/,
  NSPrivacyAccessedAPICategoryDiskSpace:
    /\b(volumeAvailableCapacity\w*|volumeTotalCapacity|systemFreeSize|systemSize|NSFileSystemFreeSize|statfs|statvfs)\b/,
  NSPrivacyAccessedAPICategoryActiveKeyboards: /\bactiveInputModes\b/,
}

type PlistValue = string | boolean | PlistValue[] | { [key: string]: PlistValue }

function plistValue(node: Element): PlistValue {
  switch (node.tagName) {
    case 'true':
      return true
    case 'false':
      return false
    case 'array':
      return [...node.children].map(plistValue)
    case 'dict': {
      const dict: { [key: string]: PlistValue } = {}
      const children = [...node.children]
      for (let at = 0; at < children.length; at += 2) {
        dict[children[at].textContent ?? ''] = plistValue(children[at + 1])
      }
      return dict
    }
    default:
      return node.textContent ?? ''
  }
}

function readPlist(path: string): { [key: string]: PlistValue } {
  const doc = new DOMParser().parseFromString(readFileSync(path, 'utf8'), 'application/xml')
  return plistValue(doc.documentElement.firstElementChild!) as { [key: string]: PlistValue }
}

/** The text of each Swift and Objective-C source under `dir`. */
function sources(dir: string): string[] {
  if (!existsSync(dir)) return []
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) return sources(path)
    return /\.(swift|m|mm|h)$/.test(name) ? [readFileSync(path, 'utf8')] : []
  })
}

/** The required-reason API categories that `texts` call. */
function categoriesIn(texts: string[]): string[] {
  return Object.entries(requiredReasonApis)
    .filter(([, symbols]) => texts.some((text) => symbols.test(text)))
    .map(([category]) => category)
}

/** The local Swift packages of the Capacitor plugins, from the package
 *  that `npx cap sync ios` writes. */
function pluginPackages(): string[] {
  const manifest = readFileSync(resolve(ios, 'CapApp-SPM/Package.swift'), 'utf8')
  return [...manifest.matchAll(/\.package\(name: "[^"]+", path: "([^"]+)"\)/g)].map((match) =>
    resolve(ios, 'CapApp-SPM', match[1]),
  )
}

describe('the privacy manifests of the iOS app', () => {
  for (const bundle of bundles) {
    describe(bundle.name, () => {
      const manifest = readPlist(resolve(ios, bundle.manifest))

      it('does no tracking and collects no data', () => {
        expect(manifest.NSPrivacyTracking).toBe(false)
        expect(manifest.NSPrivacyTrackingDomains).toEqual([])
        expect(manifest.NSPrivacyCollectedDataTypes).toEqual([])
      })

      /** The app and the extension keep the server origin in the
       *  `UserDefaults` of the App Group `group.co.pagis.mobile`. */
      it('gives the App Group reason for UserDefaults', () => {
        expect(manifest.NSPrivacyAccessedAPITypes).toContainEqual({
          NSPrivacyAccessedAPIType: 'NSPrivacyAccessedAPICategoryUserDefaults',
          NSPrivacyAccessedAPITypeReasons: ['1C8F.1'],
        })
      })

      it('gives a reason for each required-reason API that the native code calls', () => {
        const declared = (manifest.NSPrivacyAccessedAPITypes as { [key: string]: PlistValue }[]).map(
          (entry) => entry.NSPrivacyAccessedAPIType,
        )
        const called = categoriesIn([
          ...sources(resolve(ios, 'App')),
          ...sources(resolve(ios, 'PagisNotificationService')),
        ])
        expect(called.length).toBeGreaterThan(0)
        for (const category of called) expect(declared).toContain(category)
      })
    })
  }

  it('copies each manifest into its bundle', () => {
    const project = readFileSync(resolve(ios, 'App.xcodeproj/project.pbxproj'), 'utf8')
    expect(project.match(/\/\* PrivacyInfo\.xcprivacy in Resources \*\/ = \{isa = PBXBuildFile/g)).toHaveLength(2)
  })

  /** A plugin compiles into the app binary. A plugin with no manifest of
   *  its own must call no required-reason API, or the app manifest must
   *  give its reason. */
  it('has a manifest or no required-reason API in each Capacitor plugin', () => {
    const packages = pluginPackages()
    expect(packages.length).toBeGreaterThan(0)
    for (const dir of packages) {
      expect(existsSync(resolve(dir, 'Package.swift')), `${dir} is not installed`).toBe(true)
      if (holds(dir, 'PrivacyInfo.xcprivacy')) continue
      expect(categoriesIn(sources(resolve(dir, 'ios'))), dir).toEqual([])
    }
  })
})

/** Whether the tree at `dir`, outside `node_modules`, holds a file `file`. */
function holds(dir: string, file: string): boolean {
  return readdirSync(dir).some((name) => {
    const path = join(dir, name)
    if (name === file) return true
    return name !== 'node_modules' && statSync(path).isDirectory() && holds(path, file)
  })
}
