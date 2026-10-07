import Capacitor
import UIKit
import WebKit

/// The bridge of the Mobile App. It shows the Product App of the stored
/// server, or the bundled Connect screen when no server is stored.
///
/// Capacitor reads the server URL once, when it makes the bridge. So to
/// open another server the app makes a new view controller, and with it a
/// new bridge.
final class PagisViewController: CAPBridgeViewController {
    private let store = ServerStore()
    /// The server that this bridge shows, read at launch.
    private let server: WebOrigin?
    /// The page that the bridge opens first in place of the origin, such
    /// as a Sign-In Link. Nothing stores it.
    private let firstPage: URL?
    /// The web view holds its navigation delegate weakly.
    private var navigationGuard: NavigationGuard?

    init(firstPage: URL? = nil) {
        self.server = store.server
        self.firstPage = firstPage
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) {
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
        bridge.registerPluginInstance(PagisShellPlugin())
        // The origin that the bridge shows: the server, or the app's own
        // origin on the Connect screen.
        guard let shown = WebOrigin(url: bridge.config.serverURL) else { return }
        BridgeGuard(allowing: shown, next: bridge.webViewDelegationHandler).install(in: webView)
        let navigationGuard = NavigationGuard(allowing: shown, next: bridge.webViewDelegationHandler)
        navigationGuard.install(in: webView)
        self.navigationGuard = navigationGuard
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        // Capacitor loads the server URL. A Sign-In Link takes its place.
        if let firstPage {
            webView?.load(URLRequest(url: firstPage))
        }
    }

    /// Keep the server, and start the bridge again at it.
    func open(server: WebOrigin, firstPage: URL) {
        store.server = server
        restart(firstPage: firstPage)
    }

    /// Forget the server, and start the bridge again on the Connect screen.
    func changeServer() {
        store.server = nil
        restart(firstPage: nil)
    }

    private func restart(firstPage: URL?) {
        view.window?.rootViewController = PagisViewController(firstPage: firstPage)
    }
}
