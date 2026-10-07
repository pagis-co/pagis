package co.pagis.mobile;

import java.time.Instant;
import java.time.ZoneOffset;
import java.time.format.DateTimeFormatter;
import java.util.Locale;
import java.util.concurrent.TimeUnit;

/**
 * The copy of the Session and the Session cookie of the server in the
 * cookie store of the web view.
 */
final class SessionCookies {

    /**
     * The life of a Session from its last use, which is the {@code Max-Age}
     * that the daemon gives the cookie. The cookie store gives no expiry,
     * so the copy takes this life from each read of the store.
     */
    static final long SESSION_LIFE_MILLIS = TimeUnit.DAYS.toMillis(30);

    private static final DateTimeFormatter COOKIE_DATE =
        DateTimeFormatter.ofPattern("EEE, dd MMM yyyy HH:mm:ss 'GMT'", Locale.US).withZone(ZoneOffset.UTC);

    private SessionCookies() {}

    /**
     * Read the Session cookie of the server from the store, and write or
     * delete the copy. The store answers the cookies of the URL of the
     * server alone, so a cookie of another host is not read.
     */
    static void follow(ServerOrigin origin, CookieJar jar, SessionCopy copy, long nowMillis) {
        String value = sessionValue(jar.getCookie(origin.serverUrl()));
        if (value == null) {
            copy.delete();
        } else {
            copy.write(origin, new Session(value, nowMillis + SESSION_LIFE_MILLIS));
        }
    }

    /**
     * At launch, when the store holds no Session cookie of the server and
     * the copy is live, write the copy back into the store before the first
     * load. A Session that native requests kept alive then stays signed in
     * in the web view too. The cookie has the attributes that the daemon
     * gives it.
     */
    static void restore(ServerOrigin origin, CookieJar jar, SessionCopy copy, long nowMillis) {
        Session session = copy.read(origin);
        if (session == null || !session.isLive(nowMillis)) return;
        if (sessionValue(jar.getCookie(origin.serverUrl())) != null) return;
        StringBuilder cookie = new StringBuilder()
            .append(Session.COOKIE_NAME).append('=').append(session.value)
            .append("; Path=/")
            .append("; Expires=").append(COOKIE_DATE.format(Instant.ofEpochMilli(session.expiresAtMillis)))
            .append("; HttpOnly; SameSite=Strict");
        if (origin.isHttps()) cookie.append("; Secure");
        jar.setCookie(origin.serverUrl(), cookie.toString());
    }

    /** The value of the Session cookie in a {@code Cookie} header, or null. */
    static String sessionValue(String header) {
        if (header == null) return null;
        for (String pair : header.split(";")) {
            int at = pair.indexOf('=');
            if (at > 0 && pair.substring(0, at).trim().equals(Session.COOKIE_NAME)) {
                String value = pair.substring(at + 1).trim();
                return value.isEmpty() ? null : value;
            }
        }
        return null;
    }
}
