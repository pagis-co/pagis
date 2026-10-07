import Foundation
import WebKit

/// The cookie store of the web view, as the copy of the Session reads and
/// writes it. `WKHTTPCookieStore` is one.
@MainActor
protocol CookieJar: AnyObject {
    func allCookies() async -> [HTTPCookie]
    func setCookie(_ cookie: HTTPCookie) async
}

extension WKHTTPCookieStore: CookieJar {}

/// The Session cookie of a server in a list of cookies: the cookie
/// `pagis_session` of its exact host. The daemon sets the cookie with no
/// `Domain`, so a cookie of another host, also of a subdomain, is not it.
func sessionCookie(of origin: WebOrigin, in cookies: [HTTPCookie]) -> HTTPCookie? {
    cookies.first { cookie in
        cookie.name == Session.cookieName && cookie.domain.lowercased() == origin.host
    }
}

/// Keeps the copy of the Session equal to the Session cookie of the server
/// in the cookie store of the web view. A new Session replaces the copy,
/// and a removed cookie deletes it.
@MainActor
final class SessionFollower: NSObject, WKHTTPCookieStoreObserver {
    private let origin: WebOrigin
    private let copy: SessionCopy

    init(origin: WebOrigin, copy: SessionCopy) {
        self.origin = origin
        self.copy = copy
    }

    /// Follow the cookie store from now on, and read it once now.
    func install(in store: WKHTTPCookieStore) {
        store.add(self)
        Task { await follow(store) }
    }

    nonisolated func cookiesDidChange(in cookieStore: WKHTTPCookieStore) {
        Task { @MainActor in await self.follow(cookieStore) }
    }

    /// Read the Session cookie of the server, and write or delete the copy.
    func follow(_ jar: CookieJar) async {
        let cookies = await jar.allCookies()
        guard let cookie = sessionCookie(of: origin, in: cookies), let expires = cookie.expiresDate else {
            copy.delete()
            return
        }
        let session = Session(value: cookie.value, expires: expires)
        if copy.read(for: origin) != session {
            copy.write(session, for: origin)
        }
    }
}

/// The launch of the bridge at a server.
enum SessionRestore {
    /// When the cookie store holds no Session cookie of the server and the
    /// copy is live, write the copy back into the store. A Session that
    /// native requests kept alive then stays signed in in the web view too.
    /// The cookie has the attributes that the daemon gives it.
    @MainActor
    static func restore(origin: WebOrigin, copy: SessionCopy, jar: CookieJar, now: Date = Date()) async {
        guard let session = copy.read(for: origin), session.isLive(at: now) else { return }
        guard sessionCookie(of: origin, in: await jar.allCookies()) == nil else { return }
        var properties: [HTTPCookiePropertyKey: Any] = [
            .name: Session.cookieName,
            .value: session.value,
            .domain: origin.host,
            .path: "/",
            .expires: session.expires,
            HTTPCookiePropertyKey("HttpOnly"): "TRUE",
            .sameSitePolicy: HTTPCookieStringPolicy.sameSiteStrict.rawValue,
        ]
        if origin.scheme == "https" {
            properties[.secure] = "TRUE"
        }
        guard let cookie = HTTPCookie(properties: properties) else { return }
        await jar.setCookie(cookie)
    }
}
