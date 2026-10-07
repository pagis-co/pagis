import UIKit
import WebKit

/// The guard in front of the navigation delegate of Capacitor.
///
/// Capacitor keeps a main-frame navigation in the web view when its URL
/// starts with the text of the server URL. So `https://a.example.evil.com`
/// and `https://a.example:444` load in the app for the server
/// `https://a.example`, and the app shows no address. The guard opens a
/// main-frame navigation, or a new window, to each `http` or `https` origin
/// other than the origin that the bridge shows in the system browser. It
/// passes every other navigation to the next handler (ADR-0032).
final class NavigationGuard: NSObject, WKNavigationDelegate {
    private let origin: WebOrigin
    private let next: WKNavigationDelegate
    private let openOutside: (URL) -> Void

    /// WebKit calls this form of the policy method in place of the form
    /// that the guard has, when the delegate has it. The guard says that it
    /// does not have it, so each policy decision goes through the guard.
    private static let policyWithPreferences =
        NSSelectorFromString("webView:decidePolicyForNavigationAction:preferences:decisionHandler:")
    private static let policy =
        NSSelectorFromString("webView:decidePolicyForNavigationAction:decisionHandler:")

    /// - Parameters:
    ///   - origin: The origin that the bridge shows: the server, or the
    ///     app's own origin on the Connect screen.
    ///   - next: The delegation handler of the Capacitor bridge.
    ///   - openOutside: Opens a URL in the system browser.
    init(
        allowing origin: WebOrigin,
        next: WKNavigationDelegate,
        openOutside: @escaping (URL) -> Void = { UIApplication.shared.open($0) }
    ) {
        self.origin = origin
        self.next = next
        self.openOutside = openOutside
    }

    /// Put the guard in front of the navigation delegate of this web view.
    /// The web view holds its navigation delegate weakly, so the caller
    /// keeps the guard.
    func install(in webView: WKWebView) {
        webView.navigationDelegate = self
    }

    /// Whether the guard opens this navigation in the system browser.
    func opensOutside(_ action: WKNavigationAction) -> Bool {
        let topLevel = action.targetFrame?.isMainFrame ?? true
        guard topLevel, let url = action.request.url, let scheme = url.scheme?.lowercased(),
              scheme == "https" || scheme == "http"
        else { return false }
        return WebOrigin(url: url) != origin
    }

    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        if opensOutside(navigationAction), let url = navigationAction.request.url {
            openOutside(url)
            decisionHandler(.cancel)
        } else if next.responds(to: NavigationGuard.policy) {
            next.webView?(webView, decidePolicyFor: navigationAction, decisionHandler: decisionHandler)
        } else {
            decisionHandler(.allow)
        }
    }

    // Each other method of the navigation delegate goes to the next handler
    // as it is.

    override func responds(to aSelector: Selector!) -> Bool {
        if aSelector == NavigationGuard.policyWithPreferences { return false }
        return super.responds(to: aSelector) || next.responds(to: aSelector)
    }

    override func forwardingTarget(for aSelector: Selector!) -> Any? {
        next.responds(to: aSelector) ? next : super.forwardingTarget(for: aSelector)
    }
}
