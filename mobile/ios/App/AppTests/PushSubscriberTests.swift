import XCTest
@testable import App

/// `PagisPush` registers a Push Subscription through the Push Relay
/// (ADR-0032). A fake platform gives the permission and the APNs token,
/// `StubRelay` answers for the Push Relay, and the secret items are in
/// memory.
@MainActor
final class PushSubscriberTests: XCTestCase {
    /// The VAPID Key of the server: an uncompressed P-256 point.
    private let vapidKey = Data([4] + (0..<64).map { UInt8($0) }).base64URLEncodedString()
    private let otherVapidKey = Data([4] + [UInt8](repeating: 9, count: 64)).base64URLEncodedString()

    private var steps: Steps!
    private var items: MemoryItems!
    private var platform: FakePlatform!
    private var subscriber: PushSubscriber!

    override func setUp() async throws {
        steps = Steps()
        items = MemoryItems(steps: steps)
        platform = FakePlatform(steps: steps)
        StubRelay.start(steps: steps)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubRelay.self]
        subscriber = PushSubscriber(
            relay: RelayClient(
                origin: URL(string: "https://relay.example")!,
                environment: .sandbox,
                session: URLSession(configuration: configuration)
            ),
            registrations: RelayRegistrationStore(items: items),
            keys: PushKeyStore(items: items)
        )
    }

    override func tearDown() async throws {
        StubRelay.stop()
    }

    func testSubscribeAsksThenRegistersThenMakesTheKeys() async throws {
        let subscription = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)

        XCTAssertEqual(steps.all, [
            "permission",
            "token",
            "POST /v1/registrations",
            "write \(RelayRegistrationStore.item)",
            "write \(PushKeyStore.item)",
        ])
        let body = try XCTUnwrap(StubRelay.requests.first?.json)
        XCTAssertEqual(body as NSDictionary, [
            "platform": "ios",
            "environment": "sandbox",
            "token": "token-1",
            "vapid_key": vapidKey,
        ] as NSDictionary)
        let keys = try PushKeyStore(items: items).keys()
        XCTAssertEqual(subscription, PushSubscription(
            endpoint: "https://relay.example/v1/push/id-1",
            p256dh: keys.p256dh.base64URLEncodedString(),
            auth: keys.auth.base64URLEncodedString()
        ))
    }

    func testAProductionBuildRegistersForProductionAPNs() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubRelay.self]
        let production = PushSubscriber(
            relay: RelayClient(
                origin: URL(string: "https://relay.example")!,
                environment: .production,
                session: URLSession(configuration: configuration)
            ),
            registrations: RelayRegistrationStore(items: items),
            keys: PushKeyStore(items: items)
        )

        _ = try await production.subscribe(vapidKey: vapidKey, platform: platform)

        XCTAssertEqual(StubRelay.requests.first?.json?["environment"] as? String, "production")
    }

    func testASecondSubscribeWithTheSameKeyRegistersNothing() async throws {
        let first = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)

        let second = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)

        XCTAssertEqual(second, first)
        XCTAssertEqual(StubRelay.requests.map(\.line), ["POST /v1/registrations"])
    }

    func testSubscribeWithAnotherKeyRemovesTheOldRegistrationFirst() async throws {
        let first = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)

        let second = try await subscriber.subscribe(vapidKey: otherVapidKey, platform: platform)

        XCTAssertEqual(StubRelay.requests.map(\.line), [
            "POST /v1/registrations",
            "DELETE /v1/registrations/id-1",
            "POST /v1/registrations",
        ])
        XCTAssertEqual(second.endpoint, "https://relay.example/v1/push/id-2")
        XCTAssertNotEqual(second.p256dh, first.p256dh)
    }

    func testANewTokenSendsOnePutToTheRelay() async throws {
        _ = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)

        try await subscriber.tokenChanged("token-2")
        try await subscriber.tokenChanged("token-2")

        let put = try XCTUnwrap(StubRelay.requests.last)
        XCTAssertEqual(StubRelay.requests.map(\.line), ["POST /v1/registrations", "PUT /v1/registrations/id-1"])
        XCTAssertEqual(put.authorization, "Bearer secret-1")
        XCTAssertEqual(put.json as NSDictionary?, ["token": "token-2"] as NSDictionary)
        XCTAssertEqual(try RelayRegistrationStore(items: items).read()?.token, "token-2")
    }

    /// The endpoint stays the same, so the Push Subscription of the daemon
    /// stays the same.
    func testSubscribeSendsATokenThatChangedAndKeepsTheEndpoint() async throws {
        let first = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)
        platform.nextToken = "token-2"

        let second = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)

        XCTAssertEqual(StubRelay.requests.map(\.line), ["POST /v1/registrations", "PUT /v1/registrations/id-1"])
        XCTAssertEqual(second, first)
    }

    /// The relay removes a registration when APNs says that its token is
    /// gone. The app then forgets it, and the next subscribe registers
    /// again.
    func testATokenThatTheRelayDoesNotKnowForgetsTheRegistration() async throws {
        _ = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)
        StubRelay.putStatus = 404

        try await subscriber.tokenChanged("token-2")

        XCTAssertNil(try RelayRegistrationStore(items: items).read())
        XCTAssertNil(try items.read(PushKeyStore.item))
    }

    func testUnsubscribeDeletesTheRegistrationThenTheKeys() async throws {
        _ = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)
        steps.clear()

        try await subscriber.unsubscribe()

        XCTAssertEqual(steps.all, [
            "DELETE /v1/registrations/id-1",
            "delete \(RelayRegistrationStore.item)",
            "delete \(PushKeyStore.item)",
        ])
        XCTAssertEqual(StubRelay.requests.last?.authorization, "Bearer secret-1")
        XCTAssertNil(try RelayRegistrationStore(items: items).read())
        XCTAssertNil(try items.read(PushKeyStore.item))
    }

    func testARefusedPermissionRegistersNothing() async {
        platform.allows = false

        do {
            _ = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)
            XCTFail("subscribe did not fail")
        } catch {
            XCTAssertEqual(error as? PushError, .notAllowed)
        }
        XCTAssertEqual(steps.all, ["permission"])
    }

    func testARefusedRegistrationKeepsNothing() async {
        StubRelay.postStatus = 422

        do {
            _ = try await subscriber.subscribe(vapidKey: vapidKey, platform: platform)
            XCTFail("subscribe did not fail")
        } catch {
            XCTAssertEqual(error as? PushError, .relay(status: 422))
        }
        XCTAssertTrue(items.values.isEmpty)
    }
}

// MARK: - Fakes

/// The steps of a test, in order. The stub relay writes to it from the
/// thread of the URL session.
final class Steps {
    private let lock = NSLock()
    private var list: [String] = []

    var all: [String] { lock.withLock { list } }

    func append(_ step: String) {
        lock.withLock { list.append(step) }
    }

    func clear() {
        lock.withLock { list.removeAll() }
    }
}

/// Secret items in memory. It writes each change to the steps.
final class MemoryItems: SecretItems {
    private(set) var values: [String: Data] = [:]
    private let steps: Steps?

    init(steps: Steps? = nil) {
        self.steps = steps
    }

    func read(_ name: String) throws -> Data? { values[name] }

    func write(_ data: Data, as name: String) throws {
        steps?.append("write \(name)")
        values[name] = data
    }

    func delete(_ name: String) throws {
        steps?.append("delete \(name)")
        values[name] = nil
    }
}

/// The permission and the APNs token of a phone.
@MainActor
final class FakePlatform: PushPlatform {
    var allows = true
    var nextToken = "token-1"
    private let steps: Steps

    init(steps: Steps) {
        self.steps = steps
    }

    func requestPermission() async throws -> Bool {
        steps.append("permission")
        return allows
    }

    func token() async throws -> String {
        steps.append("token")
        return nextToken
    }
}

/// The Push Relay. A registration gets the id `id-<n>`, the secret
/// `secret-<n>` and the endpoint `https://relay.example/v1/push/id-<n>`.
final class StubRelay: URLProtocol {
    struct Request {
        let line: String
        let authorization: String?
        let json: [String: Any]?
    }

    static var requests: [Request] = []
    static var postStatus = 201
    static var putStatus = 204
    private static var steps: Steps?

    static func start(steps: Steps) {
        self.steps = steps
        requests = []
        postStatus = 201
        putStatus = 204
    }

    static func stop() {
        steps = nil
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let method = request.httpMethod ?? "GET"
        let path = request.url?.path ?? ""
        let line = "\(method) \(path)"
        let body = request.httpBodyStream.map(Self.read) ?? request.httpBody
        let json = body.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        Self.requests.append(Request(
            line: line,
            authorization: request.value(forHTTPHeaderField: "Authorization"),
            json: json
        ))
        Self.steps?.append(line)

        let status: Int
        var answer = Data()
        switch method {
        case "POST":
            status = Self.postStatus
            let n = Self.requests.filter { $0.line.hasPrefix("POST") }.count
            answer = Data("""
            {"id":"id-\(n)","secret":"secret-\(n)","endpoint":"https://relay.example/v1/push/id-\(n)"}
            """.utf8)
        case "PUT":
            status = Self.putStatus
        default:
            status = 204
        }
        let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: answer)
        client?.urlProtocolDidFinishLoading(self)
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
