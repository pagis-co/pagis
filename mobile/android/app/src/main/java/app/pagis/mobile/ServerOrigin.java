package app.pagis.mobile;

import java.net.URI;
import java.net.URISyntaxException;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Locale;
import java.util.Set;

/**
 * The origin of the server that the app opens: the scheme, the host and
 * the port.
 *
 * It is an {@code https://} origin, or in a debug build an
 * {@code http://} origin on a loopback host, with no user name, no
 * password, no path, no query and no fragment.
 */
final class ServerOrigin {

    /** The hosts that name this machine and no other. */
    private static final Set<String> LOOPBACK_HOSTS = new HashSet<>(Arrays.asList("127.0.0.1", "::1", "localhost"));

    private final String scheme;
    private final String host;
    /** The port, or -1 for the default port of the scheme. */
    private final int port;

    private ServerOrigin(String scheme, String host, int port) {
        this.scheme = scheme;
        this.host = host;
        this.port = port;
    }

    /**
     * The server origin in this text, or null when the text is not one
     * that this build opens.
     */
    static ServerOrigin parse(String text, boolean debug) {
        URI uri = uri(text);
        if (uri == null || uri.getRawUserInfo() != null || uri.getRawQuery() != null || uri.getRawFragment() != null) {
            return null;
        }
        String path = uri.getRawPath();
        if (path != null && !path.isEmpty() && !path.equals("/")) return null;
        ServerOrigin origin = of(uri);
        if (origin == null) return null;
        if (origin.scheme.equals("https")) return origin;
        if (debug && origin.scheme.equals("http") && LOOPBACK_HOSTS.contains(origin.host)) return origin;
        return null;
    }

    /**
     * The server URL of the bridge: the origin with no path and no
     * trailing {@code /}. Capacitor makes the allowed origin rule of its
     * bridge from this text, and a rule with a path turns that check off.
     */
    String serverUrl() {
        String name = host.contains(":") ? "[" + host + "]" : host;
        return port == -1 ? scheme + "://" + name : scheme + "://" + name + ":" + port;
    }

    /** Whether the URL in this text is on this origin. */
    boolean matches(String text) {
        URI uri = uri(text);
        return uri != null && equals(of(uri));
    }

    /**
     * Whether a main-frame navigation to the URL in this text opens in the
     * system browser: an {@code http} or {@code https} URL of any other
     * origin. Every other navigation stays with Capacitor.
     */
    boolean opensOutside(String text) {
        URI uri = uri(text);
        if (uri == null || uri.getScheme() == null) return false;
        String scheme = uri.getScheme().toLowerCase(Locale.ROOT);
        if (!scheme.equals("https") && !scheme.equals("http")) return false;
        return !equals(of(uri));
    }

    /**
     * The path, the query and the fragment of a page on this origin, such
     * as a Sign-In Link, which the bridge appends to the server URL. Null
     * for a page of any other origin.
     */
    String startPath(String page) {
        URI uri = uri(page);
        if (uri == null || uri.getRawUserInfo() != null || !equals(of(uri))) return null;
        StringBuilder path = new StringBuilder(uri.getRawPath() == null ? "" : uri.getRawPath());
        if (uri.getRawQuery() != null) path.append('?').append(uri.getRawQuery());
        if (uri.getRawFragment() != null) path.append('#').append(uri.getRawFragment());
        return path.toString();
    }

    @Override
    public boolean equals(Object other) {
        if (!(other instanceof ServerOrigin)) return false;
        ServerOrigin that = (ServerOrigin) other;
        return scheme.equals(that.scheme) && host.equals(that.host) && port == that.port;
    }

    @Override
    public int hashCode() {
        return serverUrl().hashCode();
    }

    @Override
    public String toString() {
        return serverUrl();
    }

    private static URI uri(String text) {
        if (text == null) return null;
        try {
            return new URI(text);
        } catch (URISyntaxException ex) {
            return null;
        }
    }

    /** The origin of a URI, or null when it has no scheme or no host. */
    private static ServerOrigin of(URI uri) {
        if (uri.getScheme() == null || uri.getHost() == null || uri.getHost().isEmpty()) return null;
        String scheme = uri.getScheme().toLowerCase(Locale.ROOT);
        String host = uri.getHost().toLowerCase(Locale.ROOT);
        if (host.startsWith("[") && host.endsWith("]")) host = host.substring(1, host.length() - 1);
        int port = uri.getPort();
        if ((scheme.equals("https") && port == 443) || (scheme.equals("http") && port == 80)) port = -1;
        return new ServerOrigin(scheme, host, port);
    }
}
