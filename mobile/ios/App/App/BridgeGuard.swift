import Foundation
import WebKit

/// The guard in front of the Capacitor bridge.
///
/// Capacitor injects its scripts in the main frame only, but each frame of
/// the page can post to `window.webkit.messageHandlers.bridge` and can call
/// `prompt()`, which Capacitor reads for its cookie and HTTP calls. The
/// handlers of Capacitor check neither the frame nor the origin. So the
/// guard takes the place of the `bridge` message handler and of the UI
/// delegate, and passes a call to the next handler only from the main
/// frame of the origin that the bridge shows (ADR-0032).
final class BridgeGuard: NSObject, WKScriptMessageHandler, WKUIDelegate {
    /// The name of the message handler of the Capacitor bridge.
    static let handlerName = "bridge"

    private let origin: WebOrigin
    private let next: WKScriptMessageHandler & WKUIDelegate

    /// - Parameters:
    ///   - origin: The origin that the bridge shows: the server, or the
    ///     app's own origin on the Connect screen.
    ///   - next: The delegation handler of the Capacitor bridge.
    init(allowing origin: WebOrigin, next: WKScriptMessageHandler & WKUIDelegate) {
        self.origin = origin
        self.next = next
    }

    /// Put the guard in front of the bridge of this web view. The content
    /// controller holds the guard; the web view holds its UI delegate
    /// weakly.
    func install(in webView: WKWebView) {
        let controller = webView.configuration.userContentController
        controller.removeScriptMessageHandler(forName: BridgeGuard.handlerName)
        controller.add(self, name: BridgeGuard.handlerName)
        webView.uiDelegate = self
    }

    /// Whether a call from this frame reaches the bridge.
    func allows(_ frame: WKFrameInfo) -> Bool {
        frame.isMainFrame && WebOrigin(frame.securityOrigin) == origin
    }

    // MARK: - WKScriptMessageHandler

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard allows(message.frameInfo) else { return }
        next.userContentController(userContentController, didReceive: message)
    }

    // MARK: - WKUIDelegate

    func webView(
        _ webView: WKWebView,
        runJavaScriptTextInputPanelWithPrompt prompt: String,
        defaultText: String?,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping (String?) -> Void
    ) {
        let selector = #selector(
            WKUIDelegate.webView(_:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:)
        )
        guard allows(frame), next.responds(to: selector) else {
            completionHandler(nil)
            return
        }
        next.webView?(
            webView,
            runJavaScriptTextInputPanelWithPrompt: prompt,
            defaultText: defaultText,
            initiatedByFrame: frame,
            completionHandler: completionHandler
        )
    }

    // Each other method of the UI delegate goes to the next handler as it is.

    override func responds(to aSelector: Selector!) -> Bool {
        super.responds(to: aSelector) || next.responds(to: aSelector)
    }

    override func forwardingTarget(for aSelector: Selector!) -> Any? {
        next.responds(to: aSelector) ? next : super.forwardingTarget(for: aSelector)
    }
}
