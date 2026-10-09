// `PagisPush`, the plugin of the Mobile App that registers a Push
// Subscription through the Push Relay (ADR-0032). The web view of the
// app has no Push API, so the Notifications section uses this plugin in
// place of `PushManager` where `Capacitor.isNativePlatform()` is true.

import { registerPlugin, type PermissionState } from '@capacitor/core'

import type { components } from '../api/schema'

export interface PagisPushPlugin {
  /** Whether the phone lets Pagis show notifications. */
  state(): Promise<{ permission: PermissionState }>
  /** Ask the phone for the permission, register with the Push Relay for
   *  the VAPID Key of the server, and answer the Push Subscription in the
   *  shape of `PushSubscription.toJSON()`. A second call with the same
   *  key answers the stored subscription while the relay knows it, and
   *  registers again when the relay does not. */
  subscribe(options: { vapidKey: string }): Promise<components['schemas']['SubscribeRequest']>
  /** Delete the registration with the Push Relay, then the keys. */
  unsubscribe(): Promise<void>
}

export const PagisPush = registerPlugin<PagisPushPlugin>('PagisPush')
