import Capacitor
import UIKit
import WebKit

/// The bridge of the Mobile App. It shows the Product App of the stored
/// server, or the bundled Connect screen when no server is stored. When
/// the web view cannot load the server, the bridge starts again on the
/// bundled Unreachable screen.
///
/// Capacitor reads the server URL once, when it makes the bridge. So to
/// open another server the app makes a new view controller, and with it a
/// new bridge.
final class PagisViewController: CAPBridgeViewController, PlaceShell, ServerShell {
    private let store: ServerStore
    private let sessionCopy: SessionCopy
    /// The server that this bridge shows, read at launch. Nil on the
    /// bundled pages.
    let server: WebOrigin?
    /// The plugin `PagisShell`. It also sends the Product App the place of
    /// a tap.
    private let shellPlugin = PagisShellPlugin()
    /// The page that the bridge opens first in place of the origin, such
    /// as a Sign-In Link. Nothing stores it.
    private let firstPage: URL?
    /// The web view holds its navigation delegate weakly.
    private var navigationGuard: NavigationGuard?
    /// The cookie store holds its observers weakly.
    private var sessionFollower: SessionFollower?
    /// The web view holds its UI delegate weakly.
    private var mediaGuard: MediaGuard?

    /// - Parameter showsServer: False for a bridge on the app's own origin
    ///   while a server is stored, such as the bridge of the Unreachable
    ///   screen.
    init(
        firstPage: URL? = nil,
        servers: ServerStore = ServerStore(),
        sessionCopy: SessionCopy = KeychainSessionCopy(),
        showsServer: Bool = true
    ) {
        store = servers
        self.sessionCopy = sessionCopy
        self.server = showsServer ? servers.server : nil
        self.firstPage = firstPage
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) {
        store = ServerStore()
        sessionCopy = KeychainSessionCopy()
        self.server = store.server
        self.firstPage = nil
        super.init(coder: coder)
    }

    override func instanceDescriptor() -> InstanceDescriptor {
        let descriptor = super.instanceDescriptor()
        descriptor.serverURL = server?.serverURL
        // No other host stays in the web view. `NavigationGuard` opens a
        // main-frame navigation to every other origin in the system browser.
        descriptor.allowedNavigationHostnames = []
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
        descriptor.appendedUserAgentString = AppBuild.userAgentToken(
            version: version,
            isPad: UIDevice.current.userInterfaceIdiom == .pad
        )
        return descriptor
    }

    override func capacitorDidLoad() {
        guard let bridge = bridge as? CapacitorBridge, let webView else { return }
        shellPlugin.shell = self
        bridge.registerPluginInstance(shellPlugin)
        NotificationTap.shared.shell = self
        bridge.registerPluginInstance(PagisPushPlugin())
        // The origin that the bridge shows: the server, or the app's own
        // origin on the Connect screen.
        guard let shown = WebOrigin(url: bridge.config.serverURL) else { return }
        BridgeGuard(allowing: shown, next: bridge.webViewDelegationHandler).install(in: webView)
        // In front of the bridge guard, which is the UI delegate now.
        mediaGuard = MediaGuard.install(in: webView, server: server)
        let navigationGuard = NavigationGuard(allowing: shown, next: bridge.webViewDelegationHandler) { [weak self] error in
            self?.loadFailed(error)
        }
        navigationGuard.install(in: webView)
        self.navigationGuard = navigationGuard
        if let server {
            let follower = SessionFollower(origin: server, copy: sessionCopy)
            follower.install(in: webView.configuration.websiteDataStore.httpCookieStore)
            sessionFollower = follower
        }
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        // Capacitor loads the server URL. A Sign-In Link takes its place.
        if let firstPage {
            webView?.load(URLRequest(url: firstPage))
        }
    }

    func navigate(to place: String) {
        shellPlugin.navigate(to: place)
    }

    /// Keep the server, and start the bridge again at it.
    func open(server: WebOrigin, firstPage: URL) {
        store.server = server
        restart(firstPage: firstPage)
    }

    /// Forget the server and the copy of the Session, and start the bridge
    /// again on the Connect screen.
    func changeServer() {
        store.server = nil
        sessionCopy.delete()
        restart(firstPage: nil)
    }

    /// The Session ended: the Person signed out, or the daemon refused the
    /// Session. The app opens the Connect screen.
    func sessionEnded() {
        changeServer()
    }

    /// The web view cannot load the server. Keep the server, and start the
    /// bridge again on the Unreachable screen.
    private func loadFailed(_ error: Error) {
        guard let server, let local = bridge?.config.localURL,
              let page = UnreachablePage.url(on: local, server: server, error: error),
              let window = leaveWindow()
        else { return }
        window.rootViewController = PagisViewController(
            firstPage: page,
            servers: store,
            sessionCopy: sessionCopy,
            showsServer: false
        )
    }

    private func restart(firstPage: URL?) {
        guard let window = leaveWindow() else { return }
        PagisViewController.launch(in: window, firstPage: firstPage)
    }

    /// Stop following the cookie store, and give the window that shows
    /// this bridge.
    private func leaveWindow() -> UIWindow? {
        if let sessionFollower {
            webView?.configuration.websiteDataStore.httpCookieStore.remove(sessionFollower)
        }
        // A view that is not loaded is in no window.
        return viewIfLoaded?.window
    }

    /// Show the bridge in `window`. Before the first load, the copy of the
    /// Session goes back into the cookie store when the store lost the
    /// cookie, so the bridge opens the server signed in. With no other
    /// first page, the bridge opens the place of a tap on a Notification
    /// that came before it.
    static func launch(in window: UIWindow, firstPage: URL? = nil) {
        Task { @MainActor in
            let server = ServerStore().server
            if let server {
                await SessionRestore.restore(
                    origin: server,
                    copy: KeychainSessionCopy(),
                    jar: WKWebsiteDataStore.default().httpCookieStore
                )
            }
            let tapped = NotificationTap.shared.takeFirstPage(on: server)
            window.rootViewController = PagisViewController(firstPage: firstPage ?? tapped)
            window.makeKeyAndVisible()
        }
    }
}
