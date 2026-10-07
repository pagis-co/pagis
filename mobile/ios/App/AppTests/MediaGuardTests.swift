import AVFoundation
import WebKit
import XCTest
@testable import App

/// The Mobile App grants the microphone to the main frame of its own
/// server alone. A Widget frame or a page of another origin gets no
/// microphone and no camera (ADR-0032).
@MainActor
final class MediaGuardTests: XCTestCase {
    private let server = WebOrigin(scheme: "https", host: "a.example", port: 0)

    func testTheMainFrameOfTheServerGetsTheMicrophone() {
        let guardian = MediaGuard(server: server, next: NextUIDelegate())

        XCTAssertEqual(guardian.decision(origin: server, isMainFrame: true, type: .microphone), .grant)
    }

    func testAnotherOriginGetsNoMicrophone() {
        let guardian = MediaGuard(server: server, next: NextUIDelegate())

        for other in [
            WebOrigin(scheme: "https", host: "b.example", port: 0),
            WebOrigin(scheme: "https", host: "a.example", port: 444),
            WebOrigin(scheme: "http", host: "a.example", port: 0),
        ] {
            XCTAssertEqual(guardian.decision(origin: other, isMainFrame: true, type: .microphone), .deny, "\(other)")
        }
    }

    func testASubframeGetsNoMicrophone() {
        let guardian = MediaGuard(server: server, next: NextUIDelegate())

        XCTAssertEqual(guardian.decision(origin: server, isMainFrame: false, type: .microphone), .deny)
    }

    func testNoFrameGetsTheCamera() {
        let guardian = MediaGuard(server: server, next: NextUIDelegate())

        for type in [WKMediaCaptureType.camera, .cameraAndMicrophone] {
            XCTAssertEqual(guardian.decision(origin: server, isMainFrame: true, type: type), .deny)
        }
    }

    /// The Connect screen shows no server, and needs no microphone.
    func testTheConnectScreenGetsNoMicrophone() {
        let guardian = MediaGuard(server: nil, next: NextUIDelegate())
        let connectScreen = WebOrigin(scheme: "capacitor", host: "localhost", port: 0)

        XCTAssertEqual(guardian.decision(origin: connectScreen, isMainFrame: true, type: .microphone), .deny)
    }

    /// Each other call of the UI delegate goes to the next delegate.
    func testEachOtherCallGoesToTheNextDelegate() {
        let next = NextUIDelegate()
        let guardian = MediaGuard(server: server, next: next)
        let alert = #selector(
            WKUIDelegate.webView(_:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:)
        )

        XCTAssertTrue(guardian.responds(to: alert))
        XCTAssertTrue(guardian.forwardingTarget(for: alert) as AnyObject === next)
    }

    /// Web Audio plays with the silent switch on only in a playback
    /// category.
    func testTheAudioSessionPlaysAndRecordsOnTheSpeaker() throws {
        let session = AVAudioSession.sharedInstance()

        try AudioSessionSetup.configure(session)

        XCTAssertEqual(session.category, .playAndRecord)
        XCTAssertTrue(session.categoryOptions.contains(.defaultToSpeaker))
    }
}

/// The delegate after the guard.
private final class NextUIDelegate: NSObject, WKUIDelegate {
    func webView(
        _ webView: WKWebView,
        runJavaScriptAlertPanelWithMessage message: String,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping () -> Void
    ) {
        completionHandler()
    }
}
