import XCTest
@testable import App

/// The origin match that the bridge guard uses: the scheme, the host and
/// the port must all match.
final class WebOriginTests: XCTestCase {
    /// What WKWebView gives as the origin of a frame of `https://a.example`.
    /// A `WKSecurityOrigin` names the default port of its scheme as 0.
    private let frameOfServer = WebOrigin(scheme: "https", host: "a.example", port: 0)

    func testTheServerOriginMatchesItsOwnFrame() throws {
        let server = try XCTUnwrap(WebOrigin(url: URL(string: "https://a.example")!))

        XCTAssertEqual(server, frameOfServer)
        XCTAssertEqual(WebOrigin(url: URL(string: "https://A.Example:443/")!), frameOfServer)
    }

    func testAnotherPortSchemeOrHostDoesNotMatch() {
        XCTAssertNotEqual(WebOrigin(url: URL(string: "https://a.example:444")!), frameOfServer)
        XCTAssertNotEqual(WebOrigin(url: URL(string: "http://a.example")!), frameOfServer)
        XCTAssertNotEqual(WebOrigin(url: URL(string: "https://b.a.example")!), frameOfServer)
        XCTAssertNotEqual(WebOrigin(scheme: "https", host: "a.example", port: 444), frameOfServer)
        XCTAssertNotEqual(WebOrigin(scheme: "http", host: "a.example", port: 0), frameOfServer)
    }

    /// The bridge takes the server URL with no path and no trailing `/`.
    func testTheServerURLHasNoPathAndNoTrailingSlash() {
        XCTAssertEqual(WebOrigin(url: URL(string: "https://a.example/")!)?.serverURL, "https://a.example")
        XCTAssertEqual(WebOrigin(url: URL(string: "https://a.example:444/")!)?.serverURL, "https://a.example:444")
        XCTAssertEqual(WebOrigin(url: URL(string: "http://[::1]:4400/")!)?.serverURL, "http://[::1]:4400")
    }

    func testAServerIsAnHttpsOriginAndNothingMore() {
        XCTAssertEqual(WebOrigin.server("https://a.example/", debug: false)?.serverURL, "https://a.example")
        XCTAssertEqual(WebOrigin.server("https://a.example", debug: false)?.serverURL, "https://a.example")

        for text in [
            "http://a.example/",
            "https://a.example/team",
            "https://a.example/?a=1",
            "https://a.example/#x",
            "https://ada:pw@a.example/",
            "ftp://a.example/",
            "a.example",
            "",
        ] {
            XCTAssertNil(WebOrigin.server(text, debug: false), text)
            XCTAssertNil(WebOrigin.server(text, debug: true), text)
        }
    }

    func testAServerOnLoopbackOverHttpPassesInADebugBuildOnly() {
        for text in ["http://127.0.0.1:4400/", "http://localhost:4400/", "http://[::1]:4400/"] {
            XCTAssertNotNil(WebOrigin.server(text, debug: true), text)
            XCTAssertNil(WebOrigin.server(text, debug: false), text)
        }
        XCTAssertNil(WebOrigin.server("http://localhost.example.com/", debug: true))
    }

    /// The first page of a server is its origin, or a page on it, such as
    /// a Sign-In Link.
    func testTheFirstPageIsOnTheOriginOfTheServer() throws {
        let server = try XCTUnwrap(WebOrigin.server("https://a.example/", debug: false))

        XCTAssertEqual(server.page("https://a.example/")?.absoluteString, "https://a.example/")
        XCTAssertEqual(server.page("https://a.example/sign-in#abc")?.absoluteString, "https://a.example/sign-in#abc")
        XCTAssertNil(server.page("https://b.example/sign-in#abc"))
        XCTAssertNil(server.page("https://a.example:444/sign-in#abc"))
        XCTAssertNil(server.page("http://a.example/sign-in#abc"))
        XCTAssertNil(server.page("https://ada@a.example/"))
        XCTAssertNil(server.page("not a url"))
    }
}
