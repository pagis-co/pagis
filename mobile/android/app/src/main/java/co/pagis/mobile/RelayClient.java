package co.pagis.mobile;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import org.json.JSONException;
import org.json.JSONObject;

/** The routes of the Push Relay that register an installation (ADR-0030). */
final class RelayClient {

    private static final int TIMEOUT_MILLIS = 10_000;

    private final String origin;

    /** @param origin The origin of the relay, with no trailing {@code /}. */
    RelayClient(String origin) {
        this.origin = origin;
    }

    /** {@code POST /v1/registrations}, which answers {@code {id, secret, endpoint}}. */
    RelayRegistration register(String token, String vapidKey) throws PushException {
        JSONObject body = new JSONObject();
        try {
            body.put("platform", "android");
            body.put("token", token);
            body.put("vapid_key", vapidKey);
        } catch (JSONException ex) {
            throw new PushException("Pagis cannot write the registration.", ex);
        }
        Answer answer = send("POST", "/v1/registrations", body, null);
        if (answer.status != 201) throw refused(answer.status);
        try {
            JSONObject registered = new JSONObject(answer.body);
            return new RelayRegistration(
                registered.getString("id"),
                registered.getString("secret"),
                registered.getString("endpoint"),
                vapidKey,
                token
            );
        } catch (JSONException ex) {
            throw new PushException("The Push Relay gave an answer that Pagis cannot read.", ex);
        }
    }

    /**
     * {@code PUT /v1/registrations/<id>}. False when the relay does not know
     * the registration.
     */
    boolean changeToken(RelayRegistration registration, String token) throws PushException {
        JSONObject body = new JSONObject();
        try {
            body.put("token", token);
        } catch (JSONException ex) {
            throw new PushException("Pagis cannot write the new token.", ex);
        }
        Answer answer = send("PUT", "/v1/registrations/" + registration.id, body, registration.secret);
        if (answer.status == 204) return true;
        if (answer.status == 404) return false;
        throw refused(answer.status);
    }

    /**
     * {@code DELETE /v1/registrations/<id>}. A registration that the relay
     * does not know is already gone.
     */
    void delete(RelayRegistration registration) throws PushException {
        Answer answer = send("DELETE", "/v1/registrations/" + registration.id, null, registration.secret);
        if (answer.status != 204 && answer.status != 404) throw refused(answer.status);
    }

    private static PushException refused(int status) {
        return new PushException("The Push Relay did not take the registration (status " + status + ").");
    }

    private Answer send(String method, String path, JSONObject body, String secret) throws PushException {
        HttpURLConnection connection = null;
        try {
            connection = (HttpURLConnection) new URL(origin + path).openConnection();
            connection.setRequestMethod(method);
            connection.setConnectTimeout(TIMEOUT_MILLIS);
            connection.setReadTimeout(TIMEOUT_MILLIS);
            connection.setInstanceFollowRedirects(false);
            connection.setUseCaches(false);
            if (secret != null) connection.setRequestProperty("Authorization", "Bearer " + secret);
            if (body != null) {
                byte[] bytes = body.toString().getBytes(StandardCharsets.UTF_8);
                connection.setRequestProperty("Content-Type", "application/json");
                connection.setDoOutput(true);
                connection.setFixedLengthStreamingMode(bytes.length);
                try (OutputStream out = connection.getOutputStream()) {
                    out.write(bytes);
                }
            }
            int status = connection.getResponseCode();
            InputStream stream = status >= 400 ? connection.getErrorStream() : connection.getInputStream();
            return new Answer(status, stream == null ? "" : readAll(stream));
        } catch (IOException ex) {
            throw new PushException("Pagis cannot reach the Push Relay: " + ex.getMessage(), ex);
        } finally {
            if (connection != null) connection.disconnect();
        }
    }

    private static String readAll(InputStream stream) throws IOException {
        try (InputStream in = stream) {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            for (int count = in.read(buffer); count != -1; count = in.read(buffer)) {
                bytes.write(buffer, 0, count);
            }
            return bytes.toString(StandardCharsets.UTF_8.name());
        }
    }

    /** The status and the body of an answer of the relay. */
    private static final class Answer {
        final int status;
        final String body;

        Answer(int status, String body) {
            this.status = status;
            this.body = body;
        }
    }
}
