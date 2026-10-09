import Foundation

/// The bundled Unreachable screen (`mobile/unreachable.html`). When the
/// web view cannot load the stored server, the bridge starts again on the
/// app's own origin at this page. The page names the server and the
/// error, and shows **Try again** and **Change server**.
enum UnreachablePage {
    /// The characters that stay as they are in a value of the query. A
    /// form query reads `+` as a space, so it goes encoded too.
    private static let queryValue = CharacterSet.urlQueryAllowed.subtracting(CharacterSet(charactersIn: "+&=?"))

    /// The page for this error of a main-frame load of `server`, on the
    /// app's own origin `local`. Nil when the error does not tell that the
    /// server did not load: a load that a later load cancelled, or a load
    /// that WebKit stopped.
    static func url(on local: URL, server: WebOrigin, error: Error) -> URL? {
        let error = error as NSError
        guard error.domain == NSURLErrorDomain, error.code != NSURLErrorCancelled,
              var components = URLComponents(url: local.appendingPathComponent("unreachable.html"), resolvingAgainstBaseURL: false)
        else { return nil }
        components.percentEncodedQueryItems = [
            URLQueryItem(name: "server", value: encode(server.serverURL)),
            URLQueryItem(name: "error", value: encode(error.localizedDescription)),
        ]
        return components.url
    }

    private static func encode(_ value: String) -> String? {
        value.addingPercentEncoding(withAllowedCharacters: queryValue)
    }
}
