import WebKit
import XCTest
@testable import App

/// A main-frame navigation to another origin opens in the system browser,
/// and only the server's exact origin stays in the web view (ADR-0032).
/// The tests run pages in a real web view, so each navigation action is
/// the one that WebKit gives.
@MainActor
final class NavigationGuardTests: XCTestCase {
    private let server = WebOrigin(scheme: "https", host: "a.example", port: 0)
    private let page = "https://a.example/"

    func testAMainFrameNavigationToAnotherOriginOpensOutside() {
        for target in ["https://a.example.evil.com/", "https://a.example:444/", "http://a.example/"] {
            let run = navigate(page: "<script>location.href = '\(target)'</script>")

            XCTAssertEqual(run.outside, [target], target)
            XCTAssertEqual(run.next, [page], target)
        }
    }

    func testANewWindowToAnotherOriginOpensOutside() {
        let run = navigate(page: "<a id='l' href='https://b.example/' target='_blank'>b</a><script>l.click()</script>")

        XCTAssertEqual(run.outside, ["https://b.example/"])
    }

    func testAMainFrameNavigationOnTheServerReachesTheNextHandler() {
        let run = navigate(page: "<script>location.href = 'https://a.example/x'</script>")

        XCTAssertEqual(run.outside, [])
        XCTAssertEqual(run.next, [page, "https://a.example/x"])
    }

    func testASubframeNavigationReachesTheNextHandler() {
        let run = navigate(page: "<iframe src='https://b.example/frame'></iframe>")

        XCTAssertEqual(run.outside, [])
        XCTAssertEqual(run.next, [page, "https://b.example/frame"])
    }

    /// Capacitor logs a failed load, and then the bridge hears of it.
    func testAFailedMainFrameLoadReachesTheNextHandlerAndThenTheBridge() {
        let next = FailureRecorder()
        var steps: [String] = []
        next.onFailure = { steps.append("next \($0)") }
        let guardian = NavigationGuard(allowing: server, next: next, openOutside: { _ in }) { error in
            steps.append("bridge \((error as? URLError)?.code.rawValue ?? 0)")
        }
        let webView = WKWebView(frame: .zero)

        guardian.webView(webView, didFailProvisionalNavigation: nil, withError: URLError(.cannotFindHost))
        guardian.webView(webView, didFail: nil, withError: URLError(.networkConnectionLost))

        XCTAssertEqual(steps, [
            "next provisional", "bridge \(URLError.cannotFindHost.rawValue)",
            "next committed", "bridge \(URLError.networkConnectionLost.rawValue)",
        ])
    }

    // MARK: - A page in a web view with the guard

    private struct Run {
        let outside: [String]
        let next: [String]
    }

    /// Load the page at the server, and wait for the one navigation that
    /// its script starts.
    private func navigate(page html: String) -> Run {
        let navigated = expectation(description: "the page navigated")
        var outside: [String] = []
        let next = NextNavigationHandler(allowing: page) { navigated.fulfill() }
        let configuration = WKWebViewConfiguration()
        // As Capacitor does it: a script may open a window.
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = true
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = next
        let guardian = NavigationGuard(allowing: server, next: next) { url in
            outside.append(url.absoluteString)
            navigated.fulfill()
        }

        guardian.install(in: webView)
        webView.loadHTMLString(html, baseURL: URL(string: page)!)
        // The first web content process of a CI runner starts slowly.
        wait(for: [navigated], timeout: 60)

        return Run(outside: outside, next: next.urls)
    }
}

/// The handler after the guard: the delegation handler of Capacitor. It
/// lets the first page load and cancels each other navigation, so no test
/// reaches the network.
private final class NextNavigationHandler: NSObject, WKNavigationDelegate {
    private let page: String
    private let onOther: () -> Void
    var urls: [String] = []

    init(allowing page: String, onOther: @escaping () -> Void) {
        self.page = page
        self.onOther = onOther
    }

    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        let url = navigationAction.request.url?.absoluteString ?? ""
        urls.append(url)
        if url == page {
            decisionHandler(.allow)
        } else {
            decisionHandler(.cancel)
            onOther()
        }
    }
}

/// The handler after the guard, which records each failed load, as
/// Capacitor logs it.
private final class FailureRecorder: NSObject, WKNavigationDelegate {
    var onFailure: (String) -> Void = { _ in }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        onFailure("provisional")
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        onFailure("committed")
    }
}
