import os
import UserNotifications

/// The delivered Notifications of the app and the badge of its icon.
/// `UNUserNotificationCenter` is one.
@MainActor
protocol NotificationTray: AnyObject {
    func deliveredRequests() async -> [UNNotificationRequest]
    func removeDeliveredNotifications(withIdentifiers identifiers: [String])
    func setBadgeCount(_ count: Int) async throws
}

extension UNUserNotificationCenter: NotificationTray {
    func deliveredRequests() async -> [UNNotificationRequest] {
        await deliveredNotifications().map(\.request)
    }
}

/// Keeps the delivered Notifications and the badge on the Needs-You Queue
/// when the app comes to the foreground (ADR-0032). The daemon sends no
/// push when an item leaves the queue, because iOS shows every push
/// (ADR-0030), so the app removes the Notifications of the items that
/// left.
@MainActor
final class StaleNotifications {
    static let app = StaleNotifications(
        queue: NeedsYouRead(),
        servers: ServerStore(),
        copy: KeychainSessionCopy(),
        tray: UNUserNotificationCenter.current()
    )

    private static let log = Logger(subsystem: "app.pagis.mobile", category: "notifications")

    private let queue: NeedsYouRead
    private let servers: ServerStore
    private let copy: SessionCopy
    private let tray: NotificationTray

    init(queue: NeedsYouRead, servers: ServerStore, copy: SessionCopy, tray: NotificationTray) {
        self.queue = queue
        self.servers = servers
        self.copy = copy
        self.tray = tray
    }

    /// Read the queue. Remove each delivered Notification whose `item` is
    /// not in it, also one with no `item`, and set the badge to the count.
    /// A `401` deletes the copy of the Session. A read that fails changes
    /// nothing.
    func clean() async {
        guard let origin = servers.server, let session = copy.read(for: origin) else { return }
        // The list comes before the read. A Notification that arrives
        // during the read can be of an item that is newer than the
        // answer, so it stays.
        let delivered = await tray.deliveredRequests()
        switch await queue.read(origin: origin, session: session) {
        case .queue(let items, let count):
            let stale = delivered
                .filter { request in !((request.content.userInfo["item"] as? String).map(items.contains) ?? false) }
                .map(\.identifier)
            if !stale.isEmpty {
                tray.removeDeliveredNotifications(withIdentifiers: stale)
            }
            do {
                try await tray.setBadgeCount(count)
            } catch {
                Self.log.error("The badge did not change to \(count): \(error.localizedDescription, privacy: .public)")
            }
        case .refused(let status):
            Self.log.error("The daemon answered \(status, privacy: .public) to the read of the Needs-You Queue.")
            if status == 401 {
                copy.delete()
            }
        case .unreadable(let reason):
            Self.log.error("The app did not read the Needs-You Queue: \(reason, privacy: .public)")
        }
    }
}
