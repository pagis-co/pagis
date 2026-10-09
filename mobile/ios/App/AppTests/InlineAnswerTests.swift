import UIKit
import UserNotifications
import XCTest
@testable import App

/// **Approve once** and **Deny** on a Notification of an Approval post the
/// decision with the copy of the Session, and a failure shows one
/// Notification (ADR-0032). `StubDecisionRoute` answers for the daemon,
/// and fakes stand in for the notification center and the background
/// tasks of the app.
@MainActor
final class InlineAnswerTests: XCTestCase {
    private let server = WebOrigin(scheme: "https", host: "pagis.example.com", port: 0)
    private let expires = Date().addingTimeInterval(86_400)
    private let defaults = UserDefaults(suiteName: "InlineAnswerTests")!

    private var steps: Steps!
    private var copy: FakeSessionCopy!
    private var notifications: FakeNotifications!
    private var tasks: FakeBackgroundTasks!

    override func setUp() async throws {
        steps = Steps()
        StubDecisionRoute.start(steps: steps)
        copy = FakeSessionCopy()
        copy.sessions[server] = Session(value: "s1", expires: expires)
        notifications = FakeNotifications(steps: steps)
        tasks = FakeBackgroundTasks(steps: steps)
        ServerStore(defaults: defaults).server = server
    }

    override func tearDown() async throws {
        StubDecisionRoute.stop()
        defaults.removePersistentDomain(forName: "InlineAnswerTests")
    }

    // MARK: - The category

    /// With **Answer on the lock screen** off, a Notification of an
    /// Approval shows no action.
    func testWithLockScreenAnswersOffTheApprovalCategoryHasNoActions() {
        let category = InlineAnswer.category(lockScreenAnswers: false)

        XCTAssertEqual(category.identifier, "approval")
        XCTAssertTrue(category.actions.isEmpty)
    }

    func testWithLockScreenAnswersOnTheApprovalCategoryHasApproveOnceAndDeny() {
        let category = InlineAnswer.category(lockScreenAnswers: true)

        XCTAssertEqual(category.identifier, "approval")
        XCTAssertEqual(category.actions.map(\.identifier), ["approve_once", "deny"])
        XCTAssertEqual(category.actions.map(\.title), ["Approve once", "Deny"])
    }

    /// An action from the lock screen asks the Person to unlock the phone,
    /// also with **Answer on the lock screen** on, and runs the app in the
    /// background with no scene.
    func testBothActionsNeedAnUnlockedPhoneAndRunInTheBackground() {
        let actions = InlineAnswer.category(lockScreenAnswers: true).actions
        XCTAssertEqual(actions.count, 2)
        guard actions.count == 2 else { return }
        for action in actions {
            XCTAssertTrue(action.options.contains(.authenticationRequired), action.identifier)
            XCTAssertFalse(action.options.contains(.foreground), action.identifier)
        }
        XCTAssertFalse(actions[0].options.contains(.destructive))
        XCTAssertTrue(actions[1].options.contains(.destructive))
    }

    /// The app registers the category of the stored setting, and only it.
    func testTheAppRegistersTheCategoryOfTheStoredSetting() {
        let categories = FakeCategories()
        let servers = ServerStore(defaults: defaults)

        servers.lockScreenAnswers = true
        InlineAnswer.registerCategory(servers: servers, in: categories)
        XCTAssertEqual(categories.registered.last?.map(\.identifier), ["approval"])
        XCTAssertEqual(categories.registered.last?.first?.actions.map(\.identifier), ["approve_once", "deny"])

        servers.lockScreenAnswers = false
        InlineAnswer.registerCategory(servers: servers, in: categories)
        XCTAssertEqual(categories.registered.last?.map(\.identifier), ["approval"])
        XCTAssertEqual(categories.registered.last?.first?.actions.count, 0)
    }

    func testOnlyTheTwoActionsGiveADecision() {
        XCTAssertEqual(ApprovalDecision(action: "approve_once"), .approved)
        XCTAssertEqual(ApprovalDecision(action: "deny"), .denied)
        XCTAssertNil(ApprovalDecision(action: UNNotificationDefaultActionIdentifier))
        XCTAssertNil(ApprovalDecision(action: UNNotificationDismissActionIdentifier))
    }

    // MARK: - The request

    /// The request has no `scope`, so the decision is `once` and writes no
    /// Allow Rule. It has no `Origin`, so the cross-origin check of the
    /// daemon passes it to the Session check.
    func testEachActionPostsItsDecisionWithTheCopyAndNoOrigin() async throws {
        for (action, decision) in [("approve_once", "approved"), ("deny", "denied")] {
            StubDecisionRoute.start(steps: steps)

            await answer(action)

            let sent = try XCTUnwrap(StubDecisionRoute.requests.first, action)
            XCTAssertEqual(StubDecisionRoute.requests.count, 1, action)
            XCTAssertEqual(sent.method, "POST", action)
            XCTAssertEqual(sent.url, "https://pagis.example.com/api/v1/requests/r-1/decision", action)
            XCTAssertEqual(sent.json as NSDictionary?, ["decision": decision] as NSDictionary, action)
            XCTAssertEqual(sent.headers["Cookie"], "pagis_session=s1", action)
            XCTAssertEqual(sent.headers["Content-Type"], "application/json", action)
            XCTAssertNil(sent.headers["Origin"], action)
            XCTAssertNil(sent.headers["Sec-Fetch-Site"], action)
        }
    }

    func testTheRequestIdIsOnePathSegment() async throws {
        await answer("deny", request: notification(requestId: "r/1 ?"))

        XCTAssertEqual(StubDecisionRoute.requests.first?.url, "https://pagis.example.com/api/v1/requests/r%2F1%20%3F/decision")
    }

    // MARK: - The answer

    /// The completion handler runs after the request ends, and the
    /// background task ends last.
    func testA200RemovesTheNotificationThenCompletes() async {
        await answer("approve_once")

        XCTAssertEqual(steps.all, [
            "begin",
            "POST /api/v1/requests/r-1/decision",
            "remove n-1",
            "completion",
            "end",
        ])
        XCTAssertEqual(copy.sessions[server]?.value, "s1")
    }

    func testA401ShowsTheFailureAndDeletesTheCopy() async throws {
        StubDecisionRoute.answer = .status(401)

        await answer("approve_once")

        try assertFailureShown()
        XCTAssertTrue(copy.sessions.isEmpty)
    }

    func testA404And409ShowTheFailureAndKeepTheCopy() async throws {
        for status in [404, 409] {
            StubDecisionRoute.start(steps: steps)
            StubDecisionRoute.answer = .status(status)
            steps.clear()

            await answer("deny")

            try assertFailureShown()
            XCTAssertEqual(copy.sessions[server]?.value, "s1", "status \(status)")
        }
    }

    func testNoNetworkShowsTheFailure() async throws {
        StubDecisionRoute.answer = .failure(.notConnectedToInternet)

        await answer("approve_once")

        try assertFailureShown()
    }

    /// The daemon does not answer in the time of the answer.
    func testATimeoutShowsTheFailure() async throws {
        StubDecisionRoute.answer = .silence

        await answer("approve_once", within: 0.5)

        try assertFailureShown()
    }

    func testTheAnswerWaitsTwentySeconds() {
        XCTAssertEqual(ApprovalAnswer.timeout, 20)
    }

    func testNoCopyOfTheSessionSendsNothingAndShowsTheFailure() async throws {
        copy.delete()

        await answer("approve_once")

        XCTAssertTrue(StubDecisionRoute.requests.isEmpty)
        try assertFailureShown(posted: false)
    }

    /// The failure opens the server when the Notification names no place.
    func testAFailureWithNoPlaceOpensTheServer() async throws {
        StubDecisionRoute.answer = .status(409)
        let request = notification(userInfo: ["request": ["id": "r-1", "actions": ["approve_once", "deny"]]])

        await answer("deny", request: request)

        XCTAssertEqual(notifications.added.first?.content.userInfo as NSDictionary?, ["navigate": "https://pagis.example.com"] as NSDictionary)
    }

    /// iOS ends the time of the app before the daemon answers. The task
    /// ends at once, and one time only.
    func testTheEndOfTheBackgroundTimeEndsTheTaskOnce() async {
        StubDecisionRoute.answer = .silence
        let answering = Task { await answer("approve_once", within: 0.5) }
        let deadline = Date().addingTimeInterval(5)
        while StubDecisionRoute.requests.isEmpty {
            guard Date() < deadline else {
                answering.cancel()
                return XCTFail("the answer sent no request")
            }
            await Task.yield()
        }

        tasks.expire()
        await answering.value

        XCTAssertEqual(steps.all, [
            "begin",
            "POST /api/v1/requests/r-1/decision",
            "end",
            "add n-1",
            "completion",
        ])
    }

    // MARK: - Helpers

    private func answer(
        _ action: String,
        request: UNNotificationRequest? = nil,
        within timeout: TimeInterval = ApprovalAnswer.timeout
    ) async {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubDecisionRoute.self]
        let inline = InlineAnswer(
            answer: ApprovalAnswer(configuration: configuration, timeout: timeout),
            servers: ServerStore(defaults: defaults),
            copy: copy,
            notifications: notifications,
            backgroundTasks: tasks
        )
        guard let decision = ApprovalDecision(action: action) else {
            return XCTFail("\(action) gives no decision")
        }
        await inline.answer(decision, to: request ?? notification()) { [steps] in
            steps?.append("completion")
        }
    }

    /// The Notification of an Approval, as the Notification Service
    /// Extension delivers it.
    private func notification(requestId: String = "r-1", userInfo: [AnyHashable: Any]? = nil) -> UNNotificationRequest {
        let content = UNMutableNotificationContent()
        content.title = "Robin"
        content.body = "Robin needs your approval\nhost_shell"
        content.threadIdentifier = "approval"
        content.categoryIdentifier = "approval"
        content.userInfo = userInfo ?? [
            "navigate": "https://pagis.example.com/c/ch-1",
            "item": "request:\(requestId)",
            "kind": "approval",
            "request": ["id": requestId, "actions": ["approve_once", "deny"]],
        ]
        return UNNotificationRequest(identifier: "n-1", content: content, trigger: nil)
    }

    /// The failure replaces the Notification: the same identifier, title
    /// and thread, the one text of every failure, no actions, and the
    /// place of the item. Then the completion handler runs.
    private func assertFailureShown(posted: Bool = true, file: StaticString = #filePath, line: UInt = #line) throws {
        let shown = try XCTUnwrap(notifications.added.last, file: file, line: line)
        XCTAssertEqual(notifications.added.count, 1, file: file, line: line)
        XCTAssertEqual(shown.identifier, "n-1", file: file, line: line)
        XCTAssertNil(shown.trigger, file: file, line: line)
        XCTAssertEqual(shown.content.title, "Robin", file: file, line: line)
        XCTAssertEqual(shown.content.body, "Pagis did not take this answer. Open Pagis to see the request.", file: file, line: line)
        XCTAssertEqual(shown.content.threadIdentifier, "approval", file: file, line: line)
        XCTAssertEqual(shown.content.categoryIdentifier, "", file: file, line: line)
        XCTAssertEqual(shown.content.userInfo as NSDictionary, ["navigate": "https://pagis.example.com/c/ch-1"] as NSDictionary, file: file, line: line)
        let request = posted ? ["POST /api/v1/requests/r-1/decision"] : []
        XCTAssertEqual(steps.all, ["begin"] + request + ["add n-1", "completion", "end"], file: file, line: line)
        notifications.added = []
    }
}

// MARK: - Fakes

/// The notification categories of the app. Each item of `registered` is
/// one call to `setNotificationCategories`.
@MainActor
final class FakeCategories: NotificationCategories {
    var registered: [[UNNotificationCategory]] = []

    func setNotificationCategories(_ categories: Set<UNNotificationCategory>) {
        registered.append(Array(categories))
    }
}

/// The notification center. It writes each change to the steps.
@MainActor
final class FakeNotifications: DeliveredNotifications {
    var added: [UNNotificationRequest] = []
    private let steps: Steps

    init(steps: Steps) {
        self.steps = steps
    }

    func removeDeliveredNotifications(withIdentifiers identifiers: [String]) {
        steps.append("remove \(identifiers.joined(separator: " "))")
    }

    func add(_ request: UNNotificationRequest) async throws {
        steps.append("add \(request.identifier)")
        added.append(request)
    }
}

/// The background tasks of the app. `expire()` does what iOS does when the
/// time of the app ends.
@MainActor
final class FakeBackgroundTasks: BackgroundTasks {
    private let steps: Steps
    private var expiration: (@MainActor @Sendable () -> Void)?

    init(steps: Steps) {
        self.steps = steps
    }

    func begin(name: String, expiration: @escaping @MainActor @Sendable () -> Void) -> UIBackgroundTaskIdentifier {
        steps.append("begin")
        self.expiration = expiration
        return UIBackgroundTaskIdentifier(rawValue: 7)
    }

    func end(_ task: UIBackgroundTaskIdentifier) {
        XCTAssertEqual(task, UIBackgroundTaskIdentifier(rawValue: 7))
        steps.append("end")
    }

    func expire() {
        expiration?()
    }
}

/// The decision route of the daemon.
final class StubDecisionRoute: URLProtocol {
    enum Answer {
        case status(Int)
        case failure(URLError.Code)
        /// No answer: the request waits until it times out.
        case silence
    }

    struct Request {
        let method: String
        let url: String
        let headers: [String: String]
        let json: [String: Any]?
    }

    private static let lock = NSLock()
    private static var recorded: [Request] = []
    private static var steps: Steps?
    static var answer = Answer.status(200)

    static var requests: [Request] { lock.withLock { recorded } }

    static func start(steps: Steps) {
        lock.withLock {
            self.steps = steps
            recorded = []
            answer = .status(200)
        }
    }

    static func stop() {
        lock.withLock { steps = nil }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let method = request.httpMethod ?? "GET"
        let body = request.httpBodyStream.map(StubDecisionRoute.read) ?? request.httpBody
        let recorded = Request(
            method: method,
            url: request.url?.absoluteString ?? "",
            headers: request.allHTTPHeaderFields ?? [:],
            json: body.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        )
        let answer = Self.lock.withLock { () -> Answer in
            Self.recorded.append(recorded)
            Self.steps?.append("\(method) \(request.url?.path(percentEncoded: true) ?? "")")
            return Self.answer
        }
        switch answer {
        case .status(let status):
            let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: nil)!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: Data("{}".utf8))
            client?.urlProtocolDidFinishLoading(self)
        case .failure(let code):
            client?.urlProtocol(self, didFailWithError: URLError(code))
        case .silence:
            break
        }
    }

    override func stopLoading() {}

    private static func read(_ stream: InputStream) -> Data {
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count <= 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }
}
