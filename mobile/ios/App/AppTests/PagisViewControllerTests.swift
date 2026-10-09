import XCTest
@testable import App

/// **Change server** in the bridge: the phone forgets the server and the
/// copy of the Session, and keeps the setting **Answer on the lock
/// screen** (ADR-0032).
@MainActor
final class PagisViewControllerTests: XCTestCase {
    private let server = WebOrigin(scheme: "https", host: "pagis.example.com", port: 0)
    private let defaults = UserDefaults(suiteName: "PagisViewControllerTests")!

    override func tearDown() async throws {
        defaults.removePersistentDomain(forName: "PagisViewControllerTests")
    }

    func testChangeServerForgetsTheServerAndTheCopyAndKeepsTheSetting() {
        let servers = ServerStore(defaults: defaults)
        servers.server = server
        servers.lockScreenAnswers = true
        let copy = FakeSessionCopy()
        copy.sessions[server] = Session(value: "s1", expires: Date().addingTimeInterval(86_400))
        let bridge = PagisViewController(servers: servers, sessionCopy: copy)

        bridge.changeServer()

        XCTAssertNil(servers.server)
        XCTAssertTrue(copy.sessions.isEmpty)
        XCTAssertTrue(servers.lockScreenAnswers)
    }

    /// The bridge of the Unreachable screen shows the app's own origin, and
    /// the stored server stays.
    func testABridgeThatDoesNotShowTheServerOpensTheAppsOwnOrigin() {
        let servers = ServerStore(defaults: defaults)
        servers.server = server

        let bridge = PagisViewController(servers: servers, sessionCopy: FakeSessionCopy(), showsServer: false)

        XCTAssertNil(bridge.server)
        XCTAssertNil(bridge.instanceDescriptor().serverURL)
        XCTAssertEqual(servers.server, server)
    }
}
