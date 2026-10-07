import Foundation
import WebKit

/// The origin of a web page: the scheme, the host and the port.
///
/// The port is 0 for the default port of the scheme, as `WKSecurityOrigin`
/// gives it, so an origin from a URL and the origin of a frame compare
/// equal when they name the same server.
struct WebOrigin: Equatable {
    let scheme: String
    let host: String
    let port: Int

    /// The hosts that name this machine and no other.
    private static let loopbackHosts: Set<String> = ["127.0.0.1", "::1", "localhost"]

    private static let defaultPorts = ["https": 443, "http": 80]

    init(scheme: String, host: String, port: Int) {
        self.scheme = scheme.lowercased()
        // `URL` gives an IPv6 host with its brackets, and
        // `WKSecurityOrigin` gives it without them.
        self.host = host.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        self.port = WebOrigin.defaultPorts[self.scheme] == port ? 0 : port
    }

    init?(url: URL) {
        guard let scheme = url.scheme, let host = url.host, !host.isEmpty else { return nil }
        self.init(scheme: scheme, host: host, port: url.port ?? 0)
    }

    /// The origin of a frame, as WebKit gives it.
    init(_ origin: WKSecurityOrigin) {
        self.init(scheme: origin.protocol, host: origin.host, port: origin.port)
    }

    /// The server URL of the bridge: the origin with no path and no
    /// trailing `/`. Capacitor on Android makes the allowed origin rule of
    /// its bridge from this text, and a rule with a path turns that check
    /// off.
    var serverURL: String {
        let host = self.host.contains(":") ? "[\(self.host)]" : self.host
        return port == 0 ? "\(scheme)://\(host)" : "\(scheme)://\(host):\(port)"
    }

    /// The origin of a server that the app opens, from the text that the
    /// Connect screen gives or that the app stored: an `https://` origin,
    /// or in a debug build an `http://` origin on a loopback host, with
    /// no user name, no password, no path, no query and no fragment.
    static func server(_ text: String, debug: Bool) -> WebOrigin? {
        guard let components = URLComponents(string: text),
              let url = components.url,
              let origin = WebOrigin(url: url),
              components.user == nil, components.password == nil,
              components.path.isEmpty || components.path == "/",
              components.query == nil, components.fragment == nil
        else { return nil }
        if origin.scheme == "https" { return origin }
        if debug && origin.scheme == "http" && loopbackHosts.contains(origin.host) { return origin }
        return nil
    }

    /// A page on this origin, such as a Sign-In Link, or nil for a page of
    /// any other origin.
    func page(_ text: String) -> URL? {
        guard let components = URLComponents(string: text),
              components.user == nil, components.password == nil,
              let url = components.url,
              WebOrigin(url: url) == self
        else { return nil }
        return url
    }
}
