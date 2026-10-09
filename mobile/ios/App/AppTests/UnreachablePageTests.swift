import WebKit
import XCTest
@testable import App

/// The web view cannot load the server: the bridge opens the bundled
/// Unreachable screen, which names the server and the error.
final class UnreachablePageTests: XCTestCase {
    private let local = URL(string: "capacitor://localhost")!
    private let server = WebOrigin(scheme: "https", host: "a.example", port: 0)

    func testAFailedLoadOpensThePageWithTheServerAndTheError() throws {
        let page = try XCTUnwrap(UnreachablePage.url(on: local, server: server, error: URLError(.cannotFindHost)))

        let components = try XCTUnwrap(URLComponents(url: page, resolvingAgainstBaseURL: false))
        XCTAssertEqual(components.scheme, "capacitor")
        XCTAssertEqual(components.host, "localhost")
        XCTAssertEqual(components.path, "/unreachable.html")
        XCTAssertEqual(components.queryItems, [
            URLQueryItem(name: "server", value: "https://a.example"),
            URLQueryItem(name: "error", value: URLError(.cannotFindHost).localizedDescription),
        ])
    }

    /// The page reads the query with `URLSearchParams`, which reads `+` as
    /// a space.
    func testTheQueryEncodesEachCharacterThatAFormQueryReads() throws {
        let error = NSError(domain: NSURLErrorDomain, code: NSURLErrorSecureConnectionFailed, userInfo: [
            NSLocalizedDescriptionKey: "a+b & c=d?",
        ])

        let page = try XCTUnwrap(UnreachablePage.url(on: local, server: server, error: error))

        XCTAssertTrue(page.absoluteString.hasSuffix("&error=a%2Bb%20%26%20c%3Dd%3F"), page.absoluteString)
    }

    /// A load that a later load replaces fails as cancelled. A load that
    /// WebKit stops, such as a download, fails with an error of WebKit.
    /// Neither one is a server that does not load.
    func testACancelledLoadOrAnErrorOfWebKitOpensNothing() {
        XCTAssertNil(UnreachablePage.url(on: local, server: server, error: URLError(.cancelled)))
        XCTAssertNil(UnreachablePage.url(on: local, server: server, error: NSError(domain: WKErrorDomain, code: 102)))
    }
}
