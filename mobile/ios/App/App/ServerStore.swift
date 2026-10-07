import Foundation

/// The origin of the server that the app opens, in the `UserDefaults` of
/// the App Group, so the Notification Service Extension reads it too. It
/// holds the origin alone and never a Sign-In Link, because a link holds a
/// secret.
struct ServerStore {
    private static let key = "serverOrigin"

    let defaults: UserDefaults

    init(defaults: UserDefaults = AppGroup.defaults) {
        self.defaults = defaults
    }

    /// The stored server, or nil when the app shows the Connect screen. A
    /// stored value that is not a server origin of this build reads as
    /// none.
    var server: WebOrigin? {
        get { defaults.string(forKey: ServerStore.key).flatMap { WebOrigin.server($0, debug: AppBuild.isDebug) } }
        nonmutating set { defaults.set(newValue?.serverURL, forKey: ServerStore.key) }
    }
}

/// The App Group that the app shares with its Notification Service
/// Extension. The entitlement `com.apple.security.application-groups` of
/// each target names it.
enum AppGroup {
    static let id = "group.co.pagis.mobile"

    static var defaults: UserDefaults {
        guard let defaults = UserDefaults(suiteName: id) else {
            preconditionFailure("UserDefaults takes no suite named \(id).")
        }
        return defaults
    }
}

/// Facts of this build of the app.
enum AppBuild {
    /// A debug build, which takes `http://` on a loopback host.
    static var isDebug: Bool {
        #if DEBUG
        return true
        #else
        return false
        #endif
    }

    /// The origin of the Push Relay: the build setting `PUSH_RELAY_ORIGIN`,
    /// which `Info.plist` gives as `PagisPushRelayOrigin`.
    static var pushRelayOrigin: URL {
        guard let text = Bundle.main.object(forInfoDictionaryKey: "PagisPushRelayOrigin") as? String,
              let url = URL(string: text), url.scheme == "https", url.host != nil
        else {
            preconditionFailure("PUSH_RELAY_ORIGIN of this build is not an https origin.")
        }
        return url
    }

    /// The token that the app appends to the `User-Agent` of the web view.
    /// The daemon reads `Pagis/` first and names the Session "Pagis on
    /// iPhone" or "Pagis on iPad". On an iPad, WKWebView sends the
    /// `User-Agent` of Safari on macOS, so the token also names the iPad.
    static func userAgentToken(version: String, isPad: Bool) -> String {
        isPad ? "Pagis/\(version) iPad" : "Pagis/\(version)"
    }
}
