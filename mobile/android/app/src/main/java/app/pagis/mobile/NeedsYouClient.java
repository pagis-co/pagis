package app.pagis.mobile;

import java.io.IOException;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.TimeUnit;
import okhttp3.HttpUrl;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * Reads the Needs-You Queue of the Person, {@code GET /api/v1/needs-you},
 * with the copy of the Session (ADR-0030). The request has no
 * {@code Origin} and no {@code Sec-Fetch-Site}, so the cross-origin check
 * of the daemon passes it to the Session check as a request from a program
 * (ADR-0024, ADR-0032).
 *
 * The request goes through OkHttp, which has no cookie store, as
 * {@link ApprovalAnswer} does. The {@link java.net.HttpURLConnection} of
 * Android reads the default {@link java.net.CookieHandler} of the process,
 * which Capacitor sets to the cookie store of the web view, and so it
 * would send a second {@code Cookie} header.
 */
final class NeedsYouClient {

    private static final long TIMEOUT_SECONDS = 10;

    /** One client for the process, so the reads share its connections and threads. */
    private static final OkHttpClient CLIENT = new OkHttpClient.Builder()
        .callTimeout(TIMEOUT_SECONDS, TimeUnit.SECONDS)
        // The copy of the Session goes to the daemon alone.
        .followRedirects(false)
        .followSslRedirects(false)
        .build();

    NeedsYouQueue read(ServerOrigin server, Session session) throws NeedsYouException {
        HttpUrl url = HttpUrl.get(server.serverUrl()).newBuilder().addPathSegments("api/v1/needs-you").build();
        Request request = new Request.Builder()
            .url(url)
            .header("Accept", "application/json")
            .header("Cookie", Session.COOKIE_NAME + "=" + session.value)
            .build();
        String body;
        try (Response response = CLIENT.newCall(request).execute()) {
            int status = response.code();
            if (status < 200 || status >= 300) {
                throw new NeedsYouException("The daemon answered " + status + " to the read of the Needs-You Queue.", status);
            }
            body = response.body().string();
        } catch (IOException ex) {
            throw new NeedsYouException("The read of the Needs-You Queue did not reach the daemon: " + ex.getMessage(), ex);
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
}
