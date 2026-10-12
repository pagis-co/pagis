import AuthenticationServices
import UIKit
import Capacitor
import Foundation
import UserNotifications

/// The bridge of the app, as the plugin `PagisShell` sees it.
/// `PagisViewController` is one.
@MainActor
protocol ServerShell: AnyObject {
    /// Keep `server`, and start the bridge again at `firstPage`.
    func open(server: WebOrigin, firstPage: URL)
    /// Forget the server and the copy of the Session, and start the bridge
    /// again on the Connect screen.
    func changeServer()
    /// The Session ended, and the app opens the Connect screen.
    func sessionEnded()
}

/// `PagisShell`, the plugin of the app target. The Connect screen calls it
/// (`mobile/src/shell.ts`), and so does the Product App
/// (`ui/src/mobileShell.ts`).
@objc(PagisShellPlugin)
final class PagisShellPlugin: CAPPlugin, CAPBridgedPlugin {
    let identifier = "PagisShellPlugin"
    let jsName = "PagisShell"
    let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "buildType", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "authenticate", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "takeSignInVerifier", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "open", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "changeServer", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "getLockScreenAnswers", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setLockScreenAnswers", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "sessionEnded", returnType: CAPPluginReturnPromise)
    ]

    /// The bridge that shows the plugin.
    weak var shell: ServerShell?
    private let signInBrowser = SystemSignInBrowser()
    /// The store of the setting **Answer on the lock screen**.
    var servers = ServerStore()
    /// The categories that the app registers again when the setting
    /// changes.
    var categories: NotificationCategories = UNUserNotificationCenter.current()

    @objc func buildType(_ call: CAPPluginCall) {
        call.resolve(["debug": AppBuild.isDebug])
    }

    /// Open sign-in in a browser with its own cookie store.
    @objc func authenticate(_ call: CAPPluginCall) {
        guard let url = URL(string: call.getString("url") ?? ""),
              let origin = WebOrigin(url: url),
              WebOrigin.server(origin.serverURL, debug: AppBuild.isDebug) != nil,
              url.user == nil, url.password == nil, url.path == "/sign-in", url.fragment == nil
        else {
            call.reject("This is not a sign-in page of a trusted server.")
            return
        }
        DispatchQueue.main.async { [self] in
            signInBrowser.window = bridge?.viewController?.view.window
            signInBrowser.open(url: url) { callback, error in
                guard let callback, error == nil else {
                    call.reject("Sign-in was closed. Connect again.")
                    return
                }
                call.resolve(["url": callback.absoluteString])
            }
        }
    }

    /// Give the verifier only to the bridge on its server and for its secret.
    @objc func takeSignInVerifier(_ call: CAPPluginCall) {
        DispatchQueue.main.async { [self] in
            let server = bridge.flatMap { WebOrigin(url: $0.config.serverURL) }
            let verifier = SignInVerifier.take(server: server, secret: call.getString("secret") ?? "")
            call.resolve(verifier.map { ["verifier": $0] } ?? [:])
        }
    }

    /// **Change server** of the You screen. The app forgets the server and
    /// the copy of the Session, and shows the Connect screen.
    @objc func changeServer(_ call: CAPPluginCall) {
        call.resolve()
        DispatchQueue.main.async { [weak self] in
            SignInVerifier.clear()
            self?.shell?.changeServer()
        }
    }

    /// The setting **Answer on the lock screen** of this phone.
    @objc func getLockScreenAnswers(_ call: CAPPluginCall) {
        call.resolve(["on": servers.lockScreenAnswers])
    }

    /// Keep the setting **Answer on the lock screen**, and register the
    /// category `approval` again with or without its actions (ADR-0032).
    @objc func setLockScreenAnswers(_ call: CAPPluginCall) {
        guard let on = call.getBool("on") else {
            call.reject("The setting needs an on or off value.")
            return
        }
        servers.lockScreenAnswers = on
        DispatchQueue.main.async { [self] in
            InlineAnswer.registerCategory(servers: servers, in: categories)
            call.resolve()
        }
    }

    /// Keep the origin, and start the bridge again at the server with the
    /// first page `opens`. The shell checks both again: the page asks, and
    /// the native side decides.
    @objc func open(_ call: CAPPluginCall) {
        guard let server = WebOrigin.server(call.getString("origin") ?? "", debug: AppBuild.isDebug) else {
            call.reject("This is not the origin of a Pagis server that the app opens.")
            return
        }
        guard let firstPage = server.page(call.getString("opens") ?? "") else {
            call.reject("The first page is not on the origin of the server.")
            return
        }
        DispatchQueue.main.async { [weak self] in
            SignInVerifier.clear()
            if let verifier = call.getString("verifier"), let secret = firstPage.fragment, firstPage.path == "/sign-in" {
                SignInVerifier.keep(server: server, secret: secret, verifier: verifier)
            }
            call.resolve()
            self?.shell?.open(server: server, firstPage: firstPage)
        }
    }

    /// Send the Product App the event `navigate` with the place of a tap on
    /// a Notification, and the Product App moves its router. The page does
    /// not load again. An event that comes before the Product App listens
    /// waits for its first listener.
    func navigate(to place: String) {
        notifyListeners("navigate", data: ["path": place], retainUntilConsumed: true)
    }

    /// The Product App says that the Session ended. The shell deletes the
    /// copy of the Session and opens the Connect screen.
    @objc func sessionEnded(_ call: CAPPluginCall) {
        call.resolve()
        DispatchQueue.main.async { [weak self] in
            SignInVerifier.clear()
            self?.shell?.sessionEnded()
        }
    }
}


/// A verifier stays in memory across the replacement of the bridge.
@MainActor
enum SignInVerifier {
    private static var pending: (server: WebOrigin, secret: String, verifier: String)?

    static func keep(server: WebOrigin, secret: String, verifier: String) {
        pending = (server, secret, verifier)
    }

    static func clear() { pending = nil }

    static func take(server: WebOrigin?, secret: String) -> String? {
        guard let value = pending, value.server == server, value.secret == secret else { return nil }
        pending = nil
        return value.verifier
    }
}

/// The authentication session is separate from Safari's cookie store.
final class SystemSignInBrowser: NSObject, ASWebAuthenticationPresentationContextProviding {
    var window: UIWindow?
    private var session: ASWebAuthenticationSession?

    func makeSession(url: URL, completion: @escaping ASWebAuthenticationSession.CompletionHandler) -> ASWebAuthenticationSession {
        let session = ASWebAuthenticationSession(url: url, callbackURLScheme: "pagis-app", completionHandler: completion)
        session.prefersEphemeralWebBrowserSession = true
        session.presentationContextProvider = self
        return session
    }

    func open(url: URL, completion: @escaping ASWebAuthenticationSession.CompletionHandler) {
        guard session == nil else {
            completion(nil, NSError(domain: "PagisSignIn", code: 1))
            return
        }
        let next = makeSession(url: url) { [weak self] callback, error in
            self?.session = nil
            completion(callback, error)
        }
        session = next
        if !next.start() {
            session = nil
            completion(nil, NSError(domain: "PagisSignIn", code: 2))
        }
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        window ?? ASPresentationAnchor()
    }
}
