import UserNotifications
import XCTest
@testable import App

/// When the app comes to the foreground, it reads the Needs-You Queue with
/// the copy of the Session, removes each delivered Notification whose item
/// left the queue, and sets the badge to the count (ADR-0032).
/// `StubNeedsYouRoute` answers for the daemon, and a fake stands in for
/// the notification center.
@MainActor
final class StaleNotificationsTests: XCTestCase {
    private let server = WebOrigin(scheme: "https", host: "pagis.example.com", port: 0)
    private let defaults = UserDefaults(suiteName: "StaleNotificationsTests")!

    private var steps: Steps!
    private var copy: FakeSessionCopy!
    private var tray: FakeNotificationTray!

    override func setUp() async throws {
        steps = Steps()
        StubNeedsYouRoute.start(steps: steps)
        copy = FakeSessionCopy()
        copy.sessions[server] = Session(value: "s1", expires: Date().addingTimeInterval(86_400))
        tray = FakeNotificationTray(steps: steps)
        tray.delivered = [
            delivered("n-1", item: "request:r-1"),
            delivered("n-2", item: "run:old"),
            delivered("n-3", item: nil),
        ]
        ServerStore(defaults: defaults).server = server
    }

    override func tearDown() async throws {
        StubNeedsYouRoute.stop()
        defaults.removePersistentDomain(forName: "StaleNotificationsTests")
    }

    /// The read has no `Origin`, so the cross-origin check of the daemon
    /// passes it to the Session check as a request from a program.
    func testTheReadSendsTheCopyOfTheSessionAndNoOrigin() async throws {
        StubNeedsYouRoute.answer = .queue(["request:r-1"])

        await clean()

        let sent = try XCTUnwrap(StubNeedsYouRoute.requests.first)
        XCTAssertEqual(StubNeedsYouRoute.requests.count, 1)
        XCTAssertEqual(sent.method, "GET")
        XCTAssertEqual(sent.url, "https://pagis.example.com/api/v1/needs-you")
        XCTAssertEqual(sent.headers["Cookie"], "pagis_session=s1")
        XCTAssertNil(sent.headers["Origin"])
        XCTAssertNil(sent.headers["Sec-Fetch-Site"])
    }

    /// A Notification with no item, such as the placeholder, names no
    /// item of the queue, so it goes too. The app lists the delivered
    /// Notifications before it reads the queue: a Notification that
    /// arrives during the read is not in the list, so it stays, because
    /// its item can be newer than the answer.
    func testItRemovesEachNotificationWhoseItemIsNotInTheQueueAndSetsTheBadge() async {
        StubNeedsYouRoute.answer = .queue(["request:r-1", "call:c-1"])

        await clean()

        XCTAssertEqual(steps.all, ["list", "GET /api/v1/needs-you", "remove n-2 n-3", "badge 2"])
    }

    func testAnEmptyQueueRemovesEveryNotificationAndClearsTheBadge() async {
        StubNeedsYouRoute.answer = .queue([])

        await clean()

        XCTAssertEqual(steps.all, ["list", "GET /api/v1/needs-you", "remove n-1 n-2 n-3", "badge 0"])
    }

    func testNoStaleNotificationRemovesNothing() async {
        tray.delivered = [delivered("n-1", item: "request:r-1")]
        StubNeedsYouRoute.answer = .queue(["request:r-1"])

        await clean()

        XCTAssertEqual(steps.all, ["list", "GET /api/v1/needs-you", "badge 1"])
    }

    func testAFailedReadChangesNothing() async {
        for answer: StubNeedsYouRoute.Answer in [
            .status(500, body: "{}"),
            .status(404, body: "{}"),
            .failure(.notConnectedToInternet),
            .status(200, body: "not json"),
            .status(200, body: #"{"items": [{"id": 7}], "count": 1}"#),
            .status(200, body: #"{"items": []}"#),
        ] {
            StubNeedsYouRoute.start(steps: steps)
            StubNeedsYouRoute.answer = answer
            steps.clear()

            await clean()

            XCTAssertEqual(steps.all, ["list", "GET /api/v1/needs-you"], "\(answer)")
            XCTAssertEqual(copy.sessions[server]?.value, "s1", "\(answer)")
        }
    }

    /// A `401` ends the Session: the app deletes the copy, as each native
    /// request does, and changes nothing else.
    func testA401DeletesTheCopyAndChangesNothingElse() async {
        StubNeedsYouRoute.answer = .status(401, body: "{}")

        await clean()

        XCTAssertEqual(steps.all, ["list", "GET /api/v1/needs-you"])
        XCTAssertTrue(copy.sessions.isEmpty)
    }

    func testNoCopyOfTheSessionReadsNothing() async {
        copy.delete()

        await clean()

        XCTAssertEqual(steps.all, [])
    }

    func testNoServerReadsNothing() async {
        ServerStore(defaults: defaults).server = nil

        await clean()

        XCTAssertEqual(steps.all, [])
    }

    // MARK: - Helpers

    private func clean() async {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubNeedsYouRoute.self]
        let stale = StaleNotifications(
            queue: NeedsYouRead(configuration: configuration),
            servers: ServerStore(defaults: defaults),
            copy: copy,
            tray: tray
        )
        await stale.clean()
    }

    /// A Notification as the Notification Service Extension delivers it.
    private func delivered(_ id: String, item: String?) -> UNNotificationRequest {
        let content = UNMutableNotificationContent()
        content.title = "Robin"
        content.userInfo = item.map { ["navigate": "https://pagis.example.com/c/ch-1", "item": $0] }
            ?? ["navigate": "https://pagis.example.com"]
        return UNNotificationRequest(identifier: id, content: content, trigger: nil)
    }
}

// MARK: - Fakes

/// The delivered Notifications and the badge. It writes each change to
/// the steps.
@MainActor
final class FakeNotificationTray: NotificationTray {
    var delivered: [UNNotificationRequest] = []
    private let steps: Steps

    init(steps: Steps) {
        self.steps = steps
    }

    func deliveredRequests() async -> [UNNotificationRequest] {
        steps.append("list")
        return delivered
    }

    func removeDeliveredNotifications(withIdentifiers identifiers: [String]) {
        steps.append("remove \(identifiers.joined(separator: " "))")
    }

    func setBadgeCount(_ count: Int) async throws {
        steps.append("badge \(count)")
    }
}

/// The Needs-You route of the daemon.
final class StubNeedsYouRoute: URLProtocol {
    enum Answer {
        case status(Int, body: String)
        case failure(URLError.Code)

        /// A queue with these item ids, and their count.
        static func queue(_ ids: [String]) -> Answer {
            let items = ids.map { #"{"kind": "failed", "id": "\#($0)"}"# }.joined(separator: ", ")
            return .status(200, body: #"{"items": [\#(items)], "count": \#(ids.count)}"#)
        }
    }

    struct Request {
        let method: String
        let url: String
        let headers: [String: String]
    }

    private static let lock = NSLock()
    private static var recorded: [Request] = []
    private static var steps: Steps?
    static var answer = Answer.status(200, body: #"{"items": [], "count": 0}"#)

    static var requests: [Request] { lock.withLock { recorded } }

    static func start(steps: Steps) {
        lock.withLock {
            self.steps = steps
            recorded = []
            answer = .status(200, body: #"{"items": [], "count": 0}"#)
        }
    }

    static func stop() {
        lock.withLock { steps = nil }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let method = request.httpMethod ?? "GET"
        let recorded = Request(
            method: method,
            url: request.url?.absoluteString ?? "",
            headers: request.allHTTPHeaderFields ?? [:]
        )
        let answer = Self.lock.withLock { () -> Answer in
            Self.recorded.append(recorded)
            Self.steps?.append("\(method) \(request.url?.path(percentEncoded: true) ?? "")")
            return Self.answer
        }
        switch answer {
        case .status(let status, let body):
            let response = HTTPURLResponse(
                url: request.url!,
                statusCode: status,
                httpVersion: "HTTP/1.1",
                headerFields: ["Content-Type": "application/json"]
            )!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: Data(body.utf8))
            client?.urlProtocolDidFinishLoading(self)
        case .failure(let code):
            client?.urlProtocol(self, didFailWithError: URLError(code))
        }
    }

    override func stopLoading() {}
}
