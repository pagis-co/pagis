import UserNotifications

/// The bridge of the app, as a tap on a Notification sees it.
@MainActor
protocol PlaceShell: AnyObject {
    /// The server that the bridge shows, or nil on the Connect screen.
    var server: WebOrigin? { get }
    /// Move the router of the Product App to `place`, with no new load.
    func navigate(to place: String)
}

/// A tap on the body of a Notification opens the place of its item, by the
/// rule of `WebOrigin.place(of:)` (ADR-0032). A Notification that the app
/// posts itself, such as the failure of an answer, holds `navigate` too,
/// and a tap on it follows the same rule.
@MainActor
final class NotificationTap {
    static let shared = NotificationTap(servers: ServerStore())

    /// The bridge that shows the Product App now.
    weak var shell: PlaceShell?

    private let servers: ServerStore
    /// The page that the next bridge opens first: the place of a tap that
    /// came before a bridge of the server.
    private var firstPage: URL?
    /// The last tap that this app opened. On a cold start the scene and
    /// then the delegate of the notification center give the same tap.
    private var lastTap: String?

    init(servers: ServerStore) {
        self.servers = servers
    }

    /// Open the place of `notification`.
    func open(_ notification: UNNotification) {
        open(
            navigate: notification.request.content.userInfo["navigate"],
            tap: "\(notification.request.identifier) \(notification.date.timeIntervalSince1970)"
        )
    }

    /// Open the place of `navigate` on the stored server. The bridge of the
    /// server moves the router of the Product App. With no such bridge, as
    /// on a cold start, the next bridge opens the place first.
    func open(navigate: Any?, tap: String) {
        guard tap != lastTap else { return }
        lastTap = tap
        guard let server = servers.server else { return }
        let place = server.place(of: navigate)
        if let shell, shell.server == server {
            shell.navigate(to: place)
        } else {
            firstPage = URL(string: server.serverURL + place)
        }
    }

    /// The page that a new bridge of `server` opens first, one time only.
    func takeFirstPage(on server: WebOrigin?) -> URL? {
        defer { firstPage = nil }
        return firstPage.flatMap { server?.page($0.absoluteString) }
    }
}
