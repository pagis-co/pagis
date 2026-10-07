package co.pagis.mobile;

import java.util.Objects;

/**
 * The registration of this installation with the Push Relay (ADR-0030):
 * what the relay answered, and the VAPID Key and the token that the app
 * registered.
 */
final class RelayRegistration {

    final String id;
    /** Changes the token and removes the registration. Only the app holds it. */
    final String secret;
    /** The Web Push endpoint that the daemon posts to. */
    final String endpoint;
    final String vapidKey;
    final String token;

    RelayRegistration(String id, String secret, String endpoint, String vapidKey, String token) {
        this.id = Objects.requireNonNull(id);
        this.secret = Objects.requireNonNull(secret);
        this.endpoint = Objects.requireNonNull(endpoint);
        this.vapidKey = Objects.requireNonNull(vapidKey);
        this.token = Objects.requireNonNull(token);
    }

    RelayRegistration withToken(String token) {
        return new RelayRegistration(id, secret, endpoint, vapidKey, token);
    }

    @Override
    public String toString() {
        // The secret and the token stay out of logs.
        return "RelayRegistration(" + id + ")";
    }
}
