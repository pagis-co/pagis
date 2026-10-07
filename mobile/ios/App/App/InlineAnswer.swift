import os
import UIKit
import UserNotifications

/// The notifications of the app that an answer changes.
/// `UNUserNotificationCenter` is one.
@MainActor
protocol DeliveredNotifications: AnyObject {
    func removeDeliveredNotifications(withIdentifiers identifiers: [String])
    func add(_ request: UNNotificationRequest) async throws
}

extension UNUserNotificationCenter: DeliveredNotifications {}

/// The background tasks of the app, which keep the app running while an
/// answer waits for the daemon.
@MainActor
protocol BackgroundTasks: AnyObject {
    func begin(name: String, expiration: @escaping @MainActor @Sendable () -> Void) -> UIBackgroundTaskIdentifier
    func end(_ task: UIBackgroundTaskIdentifier)
}

/// The background tasks of `UIApplication`.
@MainActor
final class AppBackgroundTasks: BackgroundTasks {
    func begin(name: String, expiration: @escaping @MainActor @Sendable () -> Void) -> UIBackgroundTaskIdentifier {
        UIApplication.shared.beginBackgroundTask(withName: name, expirationHandler: expiration)
    }

    func end(_ task: UIBackgroundTaskIdentifier) {
        UIApplication.shared.endBackgroundTask(task)
    }
}

/// **Approve once** and **Deny** on a Notification of an Approval
/// (ADR-0032). Neither action opens the app: iOS runs it in the
/// background, with no scene and no bridge, so the answer goes from native
/// code with the copy of the Session. The app keeps no record of the
/// answer. The daemon records the decision (ADR-0004).
@MainActor
final class InlineAnswer {
    /// The text of the Notification that replaces one whose answer did not
    /// go through. The service worker shows the same text.
    static let failureText = "Pagis did not take this answer. Open Pagis to see the request."

    /// The category `approval`, which the Notification Service Extension
    /// gives to a Notification of an Approval. Each action asks the Person
    /// to unlock the phone, and no action opens the app.
    static let category = UNNotificationCategory(
        identifier: NotificationContent.approvalCategory,
        actions: [
            UNNotificationAction(identifier: "approve_once", title: "Approve once", options: [.authenticationRequired]),
            UNNotificationAction(identifier: "deny", title: "Deny", options: [.authenticationRequired, .destructive]),
        ],
        intentIdentifiers: []
    )

    static let app = InlineAnswer(
        answer: ApprovalAnswer(),
        servers: ServerStore(),
        copy: KeychainSessionCopy(),
        notifications: UNUserNotificationCenter.current(),
        backgroundTasks: AppBackgroundTasks()
    )

    private static let log = Logger(subsystem: "co.pagis.mobile", category: "answer")

    private let approvalAnswer: ApprovalAnswer
    private let servers: ServerStore
    private let copy: SessionCopy
    private let notifications: DeliveredNotifications
    private let backgroundTasks: BackgroundTasks

    init(
        answer: ApprovalAnswer,
        servers: ServerStore,
        copy: SessionCopy,
        notifications: DeliveredNotifications,
        backgroundTasks: BackgroundTasks
    ) {
        approvalAnswer = answer
        self.servers = servers
        self.copy = copy
        self.notifications = notifications
        self.backgroundTasks = backgroundTasks
    }

    /// Post `decision` to the Request of the Notification `request`. When
    /// the daemon takes it, remove the Notification. Else show the failure
    /// in its place. Then call `completion`, the completion handler of the
    /// notification delegate, after which iOS can suspend the app.
    func answer(_ decision: ApprovalDecision, to request: UNNotificationRequest, completion: @escaping () -> Void) async {
        let task = HeldTask(backgroundTasks, name: "Answer an Approval")
        if await send(decision, for: request.content) {
            notifications.removeDeliveredNotifications(withIdentifiers: [request.identifier])
        } else {
            await showFailure(in: request)
        }
        completion()
        task.end()
    }

    /// Whether the daemon took the decision. A `401` deletes the copy of
    /// the Session, because the Session ended.
    private func send(_ decision: ApprovalDecision, for content: UNNotificationContent) async -> Bool {
        guard let request = content.userInfo["request"] as? [String: Any], let id = request["id"] as? String else {
            Self.log.error("The Notification names no Request.")
            return false
        }
        guard let origin = servers.server, let session = copy.read(for: origin) else {
            Self.log.error("The app holds no server or no copy of the Session.")
            return false
        }
        switch await approvalAnswer.post(decision, request: id, origin: origin, session: session) {
        case .taken:
            return true
        case .refused(let status):
            Self.log.error("The daemon answered \(status, privacy: .public) to the decision on a Request.")
            if status == 401 {
                copy.delete()
            }
            return false
        case .unreachable(let reason):
            Self.log.error("The decision on a Request did not reach the daemon: \(reason, privacy: .public)")
            return false
        }
    }

    /// Replace the Notification with the failure: the same identifier, the
    /// same title and thread, no actions, and a tap that opens the place of
    /// the item, where the approval card shows the state of the Request.
    private func showFailure(in request: UNNotificationRequest) async {
        let content = UNMutableNotificationContent()
        content.title = request.content.title
        content.body = Self.failureText
        content.threadIdentifier = request.content.threadIdentifier
        if let navigate = request.content.userInfo["navigate"] as? String ?? servers.server?.serverURL {
            content.userInfo = ["navigate": navigate]
        }
        do {
            try await notifications.add(UNNotificationRequest(identifier: request.identifier, content: content, trigger: nil))
        } catch {
            Self.log.error("The app did not show the failure of an answer: \(error.localizedDescription, privacy: .public)")
        }
    }
}

/// One background task, which ends one time only: at the end of the
/// answer, or when iOS ends the time of the app first.
@MainActor
private final class HeldTask {
    private let tasks: BackgroundTasks
    private var id = UIBackgroundTaskIdentifier.invalid

    init(_ tasks: BackgroundTasks, name: String) {
        self.tasks = tasks
        id = tasks.begin(name: name) { [weak self] in self?.end() }
    }

    func end() {
        guard id != .invalid else { return }
        tasks.end(id)
        id = .invalid
    }
}
