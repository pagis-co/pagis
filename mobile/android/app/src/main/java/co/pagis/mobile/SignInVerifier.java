package co.pagis.mobile;

/** A verifier stays in memory across the replacement of the bridge. */
final class SignInVerifier {
    private static String origin;
    private static String secret;
    private static String verifier;

    private SignInVerifier() {}

    static synchronized void keep(String server, String linkSecret, String linkVerifier) {
        origin = server;
        secret = linkSecret;
        verifier = linkVerifier;
    }

    static synchronized void clear() {
        origin = null;
        secret = null;
        verifier = null;
    }

    static synchronized String take(String server, String linkSecret) {
        if (origin == null || !origin.equals(server) || !secret.equals(linkSecret)) return null;
        String value = verifier;
        clear();
        return value;
    }
}
