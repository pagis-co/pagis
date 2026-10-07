import Capacitor
import Foundation
import UIKit
import UserNotifications

/// `PagisPush`, the plugin of the app target that the Notifications
/// section of the Product App calls (`ui/src/push/pagisPush.ts`). The web
/// view has no Push API, so the app registers the Push Subscription
/// (ADR-0032).
@objc(PagisPushPlugin)
final class PagisPushPlugin: CAPPlugin, CAPBridgedPlugin {
    let identifier = "PagisPushPlugin"
    let jsName = "PagisPush"
    let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "state", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "subscribe", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "unsubscribe", returnType: CAPPluginReturnPromise),
    ]

    /// Whether the phone lets Pagis show notifications, as a Capacitor
    /// permission state.
    @objc func state(_ call: CAPPluginCall) {
        Task { @MainActor in
            let settings = await UNUserNotificationCenter.current().notificationSettings()
            call.resolve(["permission": PagisPushPlugin.permission(settings.authorizationStatus)])
        }
    }

    @objc func subscribe(_ call: CAPPluginCall) {
        guard let vapidKey = call.getString("vapidKey"), !vapidKey.isEmpty else {
            call.reject("subscribe needs the VAPID Key of the server.")
            return
        }
        Task { @MainActor in
            do {
                let subscription = try await PushSubscriber.app.subscribe(vapidKey: vapidKey, platform: ApnsPlatform.shared)
                call.resolve([
                    "endpoint": subscription.endpoint,
                    "keys": ["p256dh": subscription.p256dh, "auth": subscription.auth],
                ])
            } catch {
                call.reject(error.localizedDescription)
            }
        }
    }

    @objc func unsubscribe(_ call: CAPPluginCall) {
        Task { @MainActor in
            do {
                try await PushSubscriber.app.unsubscribe()
                call.resolve()
            } catch {
                call.reject(error.localizedDescription)
            }
        }
    }

    static func permission(_ status: UNAuthorizationStatus) -> String {
        switch status {
        case .authorized, .provisional, .ephemeral: return "granted"
        case .denied: return "denied"
        case .notDetermined: return "prompt"
        @unknown default: return "prompt"
        }
    }
}

/// The permission and the APNs token of this phone. iOS gives the token
/// to the app delegate, which hands it here.
@MainActor
final class ApnsPlatform: PushPlatform {
    static let shared = ApnsPlatform()

    /// The `token()` calls that wait for the app delegate.
    private var waiting: [CheckedContinuation<String, Error>] = []

    func requestPermission() async throws -> Bool {
        try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound])
    }

    func token() async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            waiting.append(continuation)
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    /// APNs gave a token. A `token()` call that waits gets it. Else the
    /// token goes to the Push Relay when it changed.
    func didRegister(deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()
        guard waiting.isEmpty else {
            resume { $0.resume(returning: token) }
            return
        }
        Task {
            do {
                try await PushSubscriber.app.tokenChanged(token)
            } catch {
                NSLog("Pagis did not give the new APNs token to the Push Relay: %@", error.localizedDescription)
            }
        }
    }

    func didFailToRegister(error: Error) {
        resume { $0.resume(throwing: error) }
    }

    private func resume(_ answer: (CheckedContinuation<String, Error>) -> Void) {
        let calls = waiting
        waiting = []
        calls.forEach(answer)
    }
}
