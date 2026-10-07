import WebKit
import XCTest
@testable import App

/// The bridge answers only the main frame of the server's exact origin
/// (ADR-0032). The tests run pages in a real web view, so the frame and
/// the origin of each message are the ones that WebKit gives.
@MainActor
final class BridgeGuardTests: XCTestCase {
    private let server = WebOrigin(scheme: "https", host: "a.example", port: 0)

    func testAMessageFromTheMainFrameOfTheServerReachesTheNextHandler() {
        let run = load(
            page: "<script>\(post("bridge", "'main'"))\(post("done", "''"))</script>",
            at: "https://a.example/"
        )

        XCTAssertEqual(run.next.messages, ["main"])
    }

    func testAMessageFromASubframeDoesNotReachTheNextHandler() {
        // A frame of `srcdoc` has the origin of its parent, so only the
        // frame check stops it.
        let frame = "<script>\(post("bridge", "'frame'"))\(post("done", "''"))</script>"
        let run = load(page: "<iframe srcdoc=\"\(frame)\"></iframe>", at: "https://a.example/")

        XCTAssertEqual(run.next.messages, [])
    }

    func testAMessageFromTheMainFrameOfAnotherOriginDoesNotReachTheNextHandler() {
        for other in ["https://b.a.example/", "https://a.example:444/", "http://a.example/"] {
            let run = load(
                page: "<script>\(post("bridge", "'other'"))\(post("done", "''"))</script>",
                at: other
            )

            XCTAssertEqual(run.next.messages, [], other)
        }
    }

    /// Capacitor reads the cookies of the server through `prompt()`. A
    /// subframe gets no answer to a prompt.
    func testAPromptFromASubframeGetsNoAnswer() {
        let script = "<script>\(post("done", "String(prompt('{\\\"type\\\":\\\"CapacitorCookies.get\\\"}'))"))</script>"
        let main = load(page: script, at: "https://a.example/")
        let frame = load(page: "<iframe srcdoc=\"\(script.replacingOccurrences(of: "\"", with: "&quot;"))\"></iframe>", at: "https://a.example/")
        let other = load(page: script, at: "https://b.a.example/")

        XCTAssertEqual(main.done, ["cookies"])
        XCTAssertEqual(frame.done, ["null"])
        XCTAssertEqual(other.done, ["null"])
        XCTAssertEqual(frame.next.prompts + other.next.prompts, 0)
    }

    // MARK: - A page in a web view with the guard

    private struct Run {
        let next: NextHandler
        let done: [String]
    }

    private func load(page html: String, at baseURL: String) -> Run {
        let next = NextHandler()
        let done = DoneHandler(expectation(description: "the page at \(baseURL) ran"))
        let configuration = WKWebViewConfiguration()
        // As Capacitor does it: the delegation handler takes `bridge`.
        configuration.userContentController.add(next, name: "bridge")
        configuration.userContentController.add(done, name: "done")
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.uiDelegate = next

        BridgeGuard(allowing: server, next: next).install(in: webView)
        webView.loadHTMLString(html, baseURL: URL(string: baseURL)!)
        // The first web content process of a CI runner starts slowly.
        wait(for: [done.expectation], timeout: 60)

        webView.configuration.userContentController.removeAllScriptMessageHandlers()
        return Run(next: next, done: done.messages)
    }

    private func post(_ handler: String, _ value: String) -> String {
        "window.webkit.messageHandlers.\(handler).postMessage(\(value));"
    }
}

/// The handler after the guard: the delegation handler of Capacitor.
private final class NextHandler: NSObject, WKScriptMessageHandler, WKUIDelegate {
    var messages: [String] = []
    var prompts = 0

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        messages.append(message.body as? String ?? "")
    }

    func webView(
        _ webView: WKWebView,
        runJavaScriptTextInputPanelWithPrompt prompt: String,
        defaultText: String?,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping (String?) -> Void
    ) {
        prompts += 1
        completionHandler("cookies")
    }
}

/// The page posts to `done` when its script ran.
private final class DoneHandler: NSObject, WKScriptMessageHandler {
    let expectation: XCTestExpectation
    var messages: [String] = []

    init(_ expectation: XCTestExpectation) {
        self.expectation = expectation
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        messages.append(message.body as? String ?? "")
        expectation.fulfill()
    }
}
