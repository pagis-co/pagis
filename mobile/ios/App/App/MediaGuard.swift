import AVFoundation
import WebKit

/// The guard of the microphone and the camera of the web view.
///
/// Capacitor grants each request of a page for the microphone or the
/// camera, from each origin and each frame. The guard grants the
/// microphone to the main frame of the server alone, and denies every
/// other request: another origin, a subframe such as a Widget frame, and
/// each request for the camera (ADR-0032). WebKit then asks the Person
/// with the system prompt alone. The guard is the UI delegate of the web
/// view, and it passes each other call to the next delegate.
final class MediaGuard: NSObject, WKUIDelegate {
    private let server: WebOrigin?
    private let next: WKUIDelegate

    /// - Parameters:
    ///   - server: The stored server, or nil on the Connect screen.
    ///   - next: The UI delegate that was in place.
    init(server: WebOrigin?, next: WKUIDelegate) {
        self.server = server
        self.next = next
    }

    /// Put the guard in front of the UI delegate of this web view. The web
    /// view holds its UI delegate weakly, so the caller keeps the guard.
    static func install(in webView: WKWebView, server: WebOrigin?) -> MediaGuard? {
        guard let next = webView.uiDelegate else { return nil }
        let guardian = MediaGuard(server: server, next: next)
        webView.uiDelegate = guardian
        return guardian
    }

    /// The decision for a request of `origin` for `type`.
    func decision(origin: WebOrigin, isMainFrame: Bool, type: WKMediaCaptureType) -> WKPermissionDecision {
        guard let server, type == .microphone, isMainFrame, origin == server else { return .deny }
        return .grant
    }

    func webView(
        _ webView: WKWebView,
        requestMediaCapturePermissionFor origin: WKSecurityOrigin,
        initiatedByFrame frame: WKFrameInfo,
        type: WKMediaCaptureType,
        decisionHandler: @escaping (WKPermissionDecision) -> Void
    ) {
        decisionHandler(decision(origin: WebOrigin(origin), isMainFrame: frame.isMainFrame, type: type))
    }

    // Each other method of the UI delegate goes to the next delegate as it is.

    override func responds(to aSelector: Selector!) -> Bool {
        super.responds(to: aSelector) || next.responds(to: aSelector)
    }

    override func forwardingTarget(for aSelector: Selector!) -> Any? {
        next.responds(to: aSelector) ? next : super.forwardingTarget(for: aSelector)
    }
}

/// The audio session of the app.
enum AudioSessionSetup {
    /// Play and record, on the speaker. Web Audio follows the ring/silent
    /// switch unless the category is a playback category, so Listen-Live
    /// plays with the switch on.
    static func configure(_ session: AVAudioSession = .sharedInstance()) throws {
        try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker])
    }
}
