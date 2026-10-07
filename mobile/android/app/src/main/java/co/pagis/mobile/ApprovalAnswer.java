package co.pagis.mobile;

import java.io.IOException;
import java.util.concurrent.TimeUnit;
import okhttp3.HttpUrl;
import okhttp3.MediaType;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * Posts a decision to the decision route of the daemon,
 * {@code POST /api/v1/requests/{request_id}/decision}, with the copy of the
 * Session. The request has no {@code scope}, so the decision is
 * {@code once} and writes no Allow Rule. It has no {@code Origin} and no
 * {@code Sec-Fetch-Site}, so the cross-origin check of the daemon passes it
 * to the Session check as a request from a program (ADR-0024, ADR-0032).
 *
 * The request goes through OkHttp, which has no cookie store. The
 * {@link java.net.HttpURLConnection} of Android reads the default
 * {@link java.net.CookieHandler} of the process, which Capacitor sets to
 * the cookie store of the web view, and so it would send a second
 * {@code Cookie} header and keep each cookie that the daemon sets.
 */
final class ApprovalAnswer {

    /** How long an answer waits for the daemon, from start to end. The service worker and the iOS app wait as long. */
    static final long TIMEOUT_SECONDS = 20;

    private static final MediaType JSON = MediaType.get("application/json; charset=utf-8");

    /** One client for the process, so the posts share its connections and threads. */
    private static final OkHttpClient CLIENT = new OkHttpClient.Builder()
        .callTimeout(TIMEOUT_SECONDS, TimeUnit.SECONDS)
        // The copy of the Session goes to the daemon alone.
        .followRedirects(false)
        .followSslRedirects(false)
        .build();

    /**
     * Post {@code decision} to the Request {@code requestId} of {@code origin}
     * with {@code session}, and give the HTTP status of the answer.
     *
     * @throws IOException when no answer comes: no network, or no answer in
     *     {@link #TIMEOUT_SECONDS}.
     */
    int post(ApprovalDecision decision, String requestId, ServerOrigin origin, Session session) throws IOException {
        HttpUrl url = HttpUrl.get(origin.serverUrl()).newBuilder()
            .addPathSegments("api/v1/requests")
            .addPathSegment(requestId)
            .addPathSegment("decision")
            .build();
        Request request = new Request.Builder()
            .url(url)
            .header("Cookie", Session.COOKIE_NAME + "=" + session.value)
            .post(RequestBody.create(body(decision), JSON))
            .build();
        try (Response response = CLIENT.newCall(request).execute()) {
            return response.code();
        }
    }

    private static String body(ApprovalDecision decision) {
        try {
            return new JSONObject().put("decision", decision.value).toString();
        } catch (JSONException ex) {
            throw new IllegalStateException("Pagis cannot write the decision " + decision.value + ".", ex);
        }
    }
}
