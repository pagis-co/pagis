package app.pagis.mobile;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.Set;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * Reads the Needs-You Queue of the Person, {@code GET /api/v1/needs-you},
 * with the copy of the Session (ADR-0030). The request has no
 * {@code Origin} and no {@code Sec-Fetch-Site}, so the cross-origin check
 * of the daemon passes it to the Session check as a request from a program
 * (ADR-0024, ADR-0032).
 */
final class NeedsYouClient {

    private static final int TIMEOUT_MILLIS = 10_000;

    NeedsYouQueue read(ServerOrigin server, Session session) throws NeedsYouException {
        HttpURLConnection connection = null;
        String body;
        try {
            connection = (HttpURLConnection) new URL(server.serverUrl() + "/api/v1/needs-you").openConnection();
            connection.setConnectTimeout(TIMEOUT_MILLIS);
            connection.setReadTimeout(TIMEOUT_MILLIS);
            connection.setInstanceFollowRedirects(false);
            connection.setUseCaches(false);
            connection.setRequestProperty("Accept", "application/json");
            connection.setRequestProperty("Cookie", Session.COOKIE_NAME + "=" + session.value);
            int status = connection.getResponseCode();
            if (status < 200 || status >= 300) {
                throw new NeedsYouException("The daemon answered " + status + " to the read of the Needs-You Queue.", status);
            }
            body = readAll(connection.getInputStream());
        } catch (IOException ex) {
            throw new NeedsYouException("The read of the Needs-You Queue did not reach the daemon: " + ex.getMessage(), ex);
        } finally {
            if (connection != null) connection.disconnect();
        }
        return queue(body);
    }

    /** The queue in the body {@code {items: [{id, ...}], count}}. */
    private static NeedsYouQueue queue(String body) throws NeedsYouException {
        try {
            JSONObject answer = new JSONObject(body);
            JSONArray items = answer.getJSONArray("items");
            Set<String> ids = new HashSet<>();
            for (int i = 0; i < items.length(); i++) {
                Object id = items.getJSONObject(i).get("id");
                if (!(id instanceof String)) throw new JSONException("An item of the queue has no id.");
                ids.add((String) id);
            }
            return new NeedsYouQueue(ids, answer.getInt("count"));
        } catch (JSONException ex) {
            throw new NeedsYouException("The daemon gave a Needs-You Queue that Pagis cannot read.", ex);
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
}
