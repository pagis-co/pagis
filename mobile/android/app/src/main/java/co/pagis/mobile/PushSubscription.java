package co.pagis.mobile;

import java.util.Objects;

/**
 * The Push Subscription that the Product App posts to the daemon: the
 * shape of {@code PushSubscription.toJSON()}, with the keys as base64url.
 */
final class PushSubscription {

    final String endpoint;
    final String p256dh;
    final String auth;

    PushSubscription(String endpoint, String p256dh, String auth) {
        this.endpoint = endpoint;
        this.p256dh = p256dh;
        this.auth = auth;
    }

    @Override
    public boolean equals(Object other) {
        if (!(other instanceof PushSubscription)) return false;
        PushSubscription that = (PushSubscription) other;
        return endpoint.equals(that.endpoint) && p256dh.equals(that.p256dh) && auth.equals(that.auth);
    }

    @Override
    public int hashCode() {
        return Objects.hash(endpoint, p256dh, auth);
    }

    @Override
    public String toString() {
        return "PushSubscription(" + endpoint + ")";
    }
}
