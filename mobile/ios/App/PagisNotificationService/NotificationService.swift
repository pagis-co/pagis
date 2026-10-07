import os
import UserNotifications

/// The Notification Service Extension of the Mobile App (ADR-0032). APNs
/// starts it for each push of the Push Relay, because the push has
/// `mutable-content`. It decrypts the body `p` with the keys of the Push
/// Subscription in the Keychain access group, and shows the payload in
/// place of the placeholder of the relay.
final class NotificationService: UNNotificationServiceExtension {
    private let log = Logger(subsystem: "co.pagis.mobile", category: "push")
    private let lock = NSLock()
    private var contentHandler: ((UNNotificationContent) -> Void)?
    private var bestContent: UNNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        let origin = ServerStore().server?.serverURL
        lock.withLock {
            self.contentHandler = contentHandler
            bestContent = NotificationContent.placeholder(request.content, origin: origin)
        }
        deliver(NotificationContent.content(for: request.content, keys: storedKeys(), origin: origin))
    }

    /// iOS ends the extension soon. It delivers the placeholder with the
    /// server origin, the best content that it has before the decryption
    /// ends.
    override func serviceExtensionTimeWillExpire() {
        if let content = lock.withLock({ bestContent }) {
            deliver(content)
        }
    }

    private func storedKeys() -> PushKeys? {
        do {
            return try PushKeyStore(items: KeychainItems()).stored()
        } catch {
            log.error("The extension cannot read the keys: \(String(describing: error), privacy: .public)")
            return nil
        }
    }

    /// Give `content` to the content handler, one time only.
    private func deliver(_ content: UNNotificationContent) {
        let handler = lock.withLock { () -> ((UNNotificationContent) -> Void)? in
            defer { contentHandler = nil }
            return contentHandler
        }
        handler?(content)
    }
}
