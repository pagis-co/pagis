/**
 * `PagisShell`, the plugin of the app target on each platform
 * (`PagisShellPlugin` on iOS and on Android).
 */

import { registerPlugin } from '@capacitor/core'

import type { BuildType, ServerAddress } from './address'

export interface PagisShellPlugin {
  /** Which build of the app this is. */
  buildType(): Promise<BuildType>
  /**
   * Open a server. The shell keeps `origin` in native storage, and
   * nothing of `opens`, because a link holds a secret. It starts the
   * bridge again with `origin` as the server URL, and the first page is
   * `opens`.
   */
  open(address: ServerAddress): Promise<void>
}

export const PagisShell = registerPlugin<PagisShellPlugin>('PagisShell')
