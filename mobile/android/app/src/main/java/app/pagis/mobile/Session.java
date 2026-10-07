package app.pagis.mobile;

import java.util.Objects;

/**
 * The Session cookie of the server, {@code pagis_session}: its value and
 * its expiry. The value does not change during the life of the Session.
 */
final class Session {

    /** The name of the Session cookie of the daemon. */
    static final String COOKIE_NAME = "pagis_session";

    final String value;
    /** The expiry, in milliseconds since the epoch. */
    final long expiresAtMillis;

    Session(String value, long expiresAtMillis) {
        this.value = Objects.requireNonNull(value);
        this.expiresAtMillis = expiresAtMillis;
    }

    /** Whether the Session is still live at {@code nowMillis}. */
    boolean isLive(long nowMillis) {
        return expiresAtMillis > nowMillis;
    }

    @Override
    public boolean equals(Object other) {
        if (!(other instanceof Session)) return false;
        Session that = (Session) other;
        return value.equals(that.value) && expiresAtMillis == that.expiresAtMillis;
    }

    @Override
    public int hashCode() {
        return Objects.hash(value, expiresAtMillis);
    }

    @Override
    public String toString() {
        // The value is a credential, so it stays out of logs.
        return "Session(expires " + expiresAtMillis + ")";
    }
}
