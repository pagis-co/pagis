import XCTest
@testable import App

/// A tap on a Notification opens the place of its item (ADR-0032). With
/// no bridge yet, as on a cold start, the next bridge opens the place
/// first. With a bridge of the server, the Product App moves its router.
@MainActor
final class NotificationTapTests: XCTestCase {
    private let server = WebOrigin(scheme: "https", host: "pagis.example.com", port: 0)
    private let defaults = UserDefaults(suiteName: "NotificationTapTests")!

    override func setUp() async throws {
        ServerStore(defaults: defaults).server = server
    }

    override func tearDown() async throws {
        defaults.removePersistentDomain(forName: "NotificationTapTests")
    }

    func testATapBeforeTheBridgeIsTheFirstPageOfTheNextBridge() {
        let tap = NotificationTap(servers: ServerStore(defaults: defaults))

        tap.open(navigate: "https://pagis.example.com/c/ch-1?x=1", tap: "n-1")

        XCTAssertEqual(tap.takeFirstPage(on: server)?.absoluteString, "https://pagis.example.com/c/ch-1?x=1")
        XCTAssertNil(tap.takeFirstPage(on: server))
    }

    func testATapWithTheBridgeOpenMovesTheRouterOfTheProductApp() {
        let tap = NotificationTap(servers: ServerStore(defaults: defaults))
        let shell = FakePlaceShell(server: server)
        tap.shell = shell

        tap.open(navigate: "https://pagis.example.com/c/ch-1#card", tap: "n-1")

        XCTAssertEqual(shell.places, ["/c/ch-1#card"])
        XCTAssertNil(tap.takeFirstPage(on: server))
    }

    func testATapOnAPlaceOfAnotherOriginOpensTheRoot() {
        let tap = NotificationTap(servers: ServerStore(defaults: defaults))
        let shell = FakePlaceShell(server: server)
        tap.shell = shell

        tap.open(navigate: "https://evil.example/c/ch-1", tap: "n-1")
        tap.open(navigate: nil, tap: "n-2")

        XCTAssertEqual(shell.places, ["/", "/"])
    }

    func testAFirstPageOfAnotherServerIsNotOpened() {
        let tap = NotificationTap(servers: ServerStore(defaults: defaults))

        tap.open(navigate: "https://pagis.example.com/c/ch-1", tap: "n-1")

        XCTAssertNil(tap.takeFirstPage(on: WebOrigin(scheme: "https", host: "other.example", port: 0)))
        XCTAssertNil(tap.takeFirstPage(on: nil))
    }

    /// The bridge of the Connect screen shows no server.
    func testABridgeOfNoServerGetsNoPlace() {
        let tap = NotificationTap(servers: ServerStore(defaults: defaults))
        let shell = FakePlaceShell(server: nil)
        tap.shell = shell

        tap.open(navigate: "https://pagis.example.com/c/ch-1", tap: "n-1")

        XCTAssertEqual(shell.places, [])
        XCTAssertEqual(tap.takeFirstPage(on: server)?.absoluteString, "https://pagis.example.com/c/ch-1")
    }

    /// On a cold start, the scene and then the delegate give the same tap.
    func testTheSameTapOpensItsPlaceOnce() {
        let tap = NotificationTap(servers: ServerStore(defaults: defaults))
        tap.open(navigate: "https://pagis.example.com/c/ch-1", tap: "n-1")
        _ = tap.takeFirstPage(on: server)
        let shell = FakePlaceShell(server: server)
        tap.shell = shell

        tap.open(navigate: "https://pagis.example.com/c/ch-1", tap: "n-1")
        tap.open(navigate: "https://pagis.example.com/c/ch-2", tap: "n-2")

        XCTAssertEqual(shell.places, ["/c/ch-2"])
    }

    func testWithNoServerATapOpensNothing() {
        ServerStore(defaults: defaults).server = nil
        let tap = NotificationTap(servers: ServerStore(defaults: defaults))

        tap.open(navigate: "https://pagis.example.com/c/ch-1", tap: "n-1")

        XCTAssertNil(tap.takeFirstPage(on: server))
    }
}

/// A bridge that records the places it gets.
@MainActor
final class FakePlaceShell: PlaceShell {
    let server: WebOrigin?
    var places: [String] = []

    init(server: WebOrigin?) {
        self.server = server
    }

    func navigate(to place: String) {
        places.append(place)
    }
}
