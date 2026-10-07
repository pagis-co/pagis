import WebKit
import XCTest
@testable import App

/// The native copy of the Session follows the Session cookie of the server
/// in the cookie store of the web view (ADR-0032).
@MainActor
final class SessionCopyTests: XCTestCase {
    private let server = WebOrigin(scheme: "https", host: "a.example", port: 0)
    /// Ten days from now, in whole seconds. A cookie keeps its expiry to the
    /// second, and WebKit caps it at 400 days.
    private let expires = Date(timeIntervalSince1970: (Date().timeIntervalSince1970 + 10 * 86_400).rounded(.down))

    // MARK: - The follower

    func testAChangeWritesTheCopy() async {
        let copy = FakeSessionCopy()
        let jar = FakeCookieJar([cookie("pagis_session", "s1", host: "a.example")])

        await SessionFollower(origin: server, copy: copy).follow(jar)

        XCTAssertEqual(copy.sessions[server], Session(value: "s1", expires: expires))
    }

    func testANewSessionReplacesTheCopy() async {
        let copy = FakeSessionCopy()
        copy.sessions[server] = Session(value: "old", expires: expires)
        let jar = FakeCookieJar([cookie("pagis_session", "new", host: "a.example")])

        await SessionFollower(origin: server, copy: copy).follow(jar)

        XCTAssertEqual(copy.sessions[server]?.value, "new")
    }

    func testARemovedCookieDeletesTheCopy() async {
        let copy = FakeSessionCopy()
        copy.sessions[server] = Session(value: "s1", expires: expires)
        let jar = FakeCookieJar([cookie("other", "x", host: "a.example")])

        await SessionFollower(origin: server, copy: copy).follow(jar)

        XCTAssertTrue(copy.sessions.isEmpty)
    }

    func testACookieOfAnotherHostIsIgnored() async {
        let copy = FakeSessionCopy()
        let jar = FakeCookieJar([
            cookie("pagis_session", "b", host: "b.example"),
            cookie("pagis_session", "sub", host: "x.a.example"),
        ])

        await SessionFollower(origin: server, copy: copy).follow(jar)
        XCTAssertTrue(copy.sessions.isEmpty)

        jar.cookies.append(cookie("pagis_session", "a", host: "a.example"))
        await SessionFollower(origin: server, copy: copy).follow(jar)
        XCTAssertEqual(copy.sessions[server]?.value, "a")
    }

    // MARK: - The launch

    /// A Session that native requests kept alive stays signed in in the
    /// web view too.
    func testALaunchWithALiveCopyAndAnEmptyStoreWritesTheCookie() async throws {
        let copy = FakeSessionCopy()
        copy.sessions[server] = Session(value: "s1", expires: expires)
        let jar = FakeCookieJar([])

        await SessionRestore.restore(origin: server, copy: copy, jar: jar, now: expires.addingTimeInterval(-60))

        let written = try XCTUnwrap(jar.cookies.first)
        XCTAssertEqual(jar.cookies.count, 1)
        XCTAssertEqual(written.name, "pagis_session")
        XCTAssertEqual(written.value, "s1")
        XCTAssertEqual(written.domain, "a.example")
        XCTAssertEqual(written.path, "/")
        XCTAssertEqual(written.expiresDate, expires)
        XCTAssertTrue(written.isHTTPOnly)
        XCTAssertTrue(written.isSecure)
        XCTAssertEqual(written.sameSitePolicy, .sameSiteStrict)
    }

    /// A debug build reaches a daemon over `http://` on loopback, where the
    /// cookie has no `Secure`.
    func testTheCookieOfAnHttpOriginIsNotSecure() async throws {
        let loopback = WebOrigin(scheme: "http", host: "127.0.0.1", port: 4400)
        let copy = FakeSessionCopy()
        copy.sessions[loopback] = Session(value: "s1", expires: expires)
        let jar = FakeCookieJar([])

        await SessionRestore.restore(origin: loopback, copy: copy, jar: jar, now: expires.addingTimeInterval(-60))

        let written = try XCTUnwrap(jar.cookies.first)
        XCTAssertEqual(written.domain, "127.0.0.1")
        XCTAssertFalse(written.isSecure)
    }

    func testALaunchKeepsACookieThatTheStoreHolds() async {
        let copy = FakeSessionCopy()
        copy.sessions[server] = Session(value: "copy", expires: expires)
        let jar = FakeCookieJar([cookie("pagis_session", "store", host: "a.example")])

        await SessionRestore.restore(origin: server, copy: copy, jar: jar, now: expires.addingTimeInterval(-60))

        XCTAssertEqual(jar.cookies.map(\.value), ["store"])
    }

    func testALaunchWritesNoExpiredCopy() async {
        let copy = FakeSessionCopy()
        copy.sessions[server] = Session(value: "s1", expires: expires)
        let jar = FakeCookieJar([])

        await SessionRestore.restore(origin: server, copy: copy, jar: jar, now: expires)

        XCTAssertTrue(jar.cookies.isEmpty)
    }

    // MARK: - The Keychain

    func testTheKeychainCopyReadsBackWhatWasWritten() {
        let copy = KeychainSessionCopy(service: "co.pagis.mobile.session.tests")
        copy.delete()
        let other = WebOrigin(scheme: "https", host: "b.example", port: 0)

        copy.write(Session(value: "s1", expires: expires), for: server)

        XCTAssertEqual(copy.read(for: server), Session(value: "s1", expires: expires))
        XCTAssertNil(copy.read(for: other))

        copy.write(Session(value: "s2", expires: expires), for: server)
        XCTAssertEqual(copy.read(for: server)?.value, "s2")

        copy.delete()
        XCTAssertNil(copy.read(for: server))
    }

    // MARK: - Helpers

    private func cookie(_ name: String, _ value: String, host: String) -> HTTPCookie {
        HTTPCookie(properties: [
            .name: name,
            .value: value,
            .domain: host,
            .path: "/",
            .expires: expires,
        ])!
    }
}

/// A copy in memory.
final class FakeSessionCopy: SessionCopy {
    var sessions: [WebOrigin: Session] = [:]

    func read(for origin: WebOrigin) -> Session? { sessions[origin] }
    func write(_ session: Session, for origin: WebOrigin) { sessions = [origin: session] }
    func delete() { sessions = [:] }
}

/// A cookie store in memory.
@MainActor
final class FakeCookieJar: CookieJar {
    var cookies: [HTTPCookie]

    init(_ cookies: [HTTPCookie]) {
        self.cookies = cookies
    }

    func allCookies() async -> [HTTPCookie] { cookies }
    func setCookie(_ cookie: HTTPCookie) async { cookies.append(cookie) }
}
