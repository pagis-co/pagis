import Foundation
import os
import UserNotifications

/// Why a push holds no body that the app can read.
enum NotificationContentError: Error, Equatable {
    /// The push has no `p`, or `p` is not base64url.
    case noBody
    /// The app holds no keys of a Push Subscription.
    case noKeys
}

/// The content that the Notification Service Extension delivers for a
/// push of the Push Relay (ADR-0030, ADR-0032).
enum NotificationContent {
    /// The category of a Notification that answers an Approval with
    /// **Approve once** and **Deny**.
    static let approvalCategory = "approval"
    /// The actions of a Request that the `approval` category shows.
    static let approvalActions = ["approve_once", "deny"]

    private static let log = Logger(subsystem: "co.pagis.mobile", category: "push")

    /// The content of the Notification of `placeholder`, the content that
    /// APNs gives: the decrypted payload, or the placeholder with the
    /// server `origin` as its place when the push does not decrypt or
    /// does not parse.
    static func content(for placeholder: UNNotificationContent, keys: PushKeys?, origin: String?) -> UNNotificationContent {
        do {
            guard let keys else { throw NotificationContentError.noKeys }
            return shown(try payload(of: placeholder.userInfo, keys: keys), over: placeholder)
        } catch {
            log.error("The push shows the placeholder: \(String(describing: error), privacy: .private)")
            return self.placeholder(placeholder, origin: origin)
        }
    }

    /// The placeholder as it is, with the server `origin` as its place.
    static func placeholder(_ placeholder: UNNotificationContent, origin: String?) -> UNNotificationContent {
        let content = mutableCopy(of: placeholder)
        content.userInfo = origin.map { ["navigate": $0] } ?? [:]
        return content
    }

    /// The payload in the body `p` of a push.
    private static func payload(of userInfo: [AnyHashable: Any], keys: PushKeys) throws -> PushPayload {
        guard let text = userInfo["p"] as? String, let body = Data(base64URLEncoded: text) else {
            throw NotificationContentError.noBody
        }
        return try PushPayload(json: decrypt(body: body, privateKey: keys.privateKey, auth: keys.auth))
    }

    /// The placeholder with the text, the thread, the badge, the category
    /// and the place of `payload`. The sound of the placeholder stays.
    private static func shown(_ payload: PushPayload, over placeholder: UNNotificationContent) -> UNNotificationContent {
        let content = mutableCopy(of: placeholder)
        content.title = payload.title
        content.body = payload.body
        content.threadIdentifier = payload.kind
        if let badge = payload.badge {
            content.badge = NSNumber(value: badge)
        }
        var userInfo: [AnyHashable: Any] = [
            "navigate": payload.navigate,
            "item": payload.item,
            "kind": payload.kind,
        ]
        if let request = payload.request {
            userInfo["request"] = ["id": request.id, "actions": request.actions]
        }
        content.userInfo = userInfo
        content.categoryIdentifier = payload.request?.actions == approvalActions ? approvalCategory : ""
        return content
    }

    private static func mutableCopy(of content: UNNotificationContent) -> UNMutableNotificationContent {
        // `UNNotificationContent` documents `mutableCopy()` to give a
        // `UNMutableNotificationContent`.
        content.mutableCopy() as! UNMutableNotificationContent
    }
}
