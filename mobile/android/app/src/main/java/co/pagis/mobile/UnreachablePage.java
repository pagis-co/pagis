package co.pagis.mobile;

import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;

/**
 * The bundled Unreachable screen ({@code mobile/unreachable.html}). When
 * the web view cannot load the stored server, the bridge starts again on
 * the app's own origin at this page. The page names the server and the
 * error, and shows Try again and Change server.
 */
final class UnreachablePage {

    private UnreachablePage() {}

    /**
     * The start path of the page for this error of a main-frame load of
     * {@code server}, which the bridge appends to the app's own origin.
     */
    static String startPath(ServerOrigin server, String error) {
        return "/unreachable.html?server=" + encode(server.serverUrl()) + "&error=" + encode(error);
    }

    private static String encode(String value) {
        try {
            return URLEncoder.encode(value, StandardCharsets.UTF_8.name());
        } catch (java.io.UnsupportedEncodingException ex) {
            throw new AssertionError("Every Java platform has UTF-8.", ex);
        }
    }
}
