package co.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertThrows;

import java.io.IOException;
import java.net.CookieHandler;
import java.net.ServerSocket;
import java.net.URI;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import okhttp3.mockwebserver.MockResponse;
import okhttp3.mockwebserver.MockWebServer;
import okhttp3.mockwebserver.RecordedRequest;
import org.json.JSONObject;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;

/**
 * {@link ApprovalAnswer} posts the decision of a notification action to
 * the decision route of the daemon, with the copy of the Session and with
 * no {@code scope} and no {@code Origin} (ADR-0032). {@link MockWebServer}
 * answers for the daemon.
 */
public class ApprovalAnswerTest {

    @Rule
    public final MockWebServer daemon = new MockWebServer();

    private final Session session = new Session("s1", 1_900_000_000_000L);
    private ServerOrigin server;

    @Before
    public void findServer() {
        String url = daemon.url("/").toString();
        server = ServerOrigin.parse(url.substring(0, url.length() - 1), true);
    }

    @Test
    public void eachActionPostsItsDecisionWithTheCopyAndNoScopeAndNoOrigin() throws Exception {
        String[][] cases = { { "approve_once", "approved" }, { "deny", "denied" } };
        for (String[] action : cases) {
            daemon.enqueue(new MockResponse().setResponseCode(200));

            int status = new ApprovalAnswer().post(ApprovalDecision.ofAction(action[0]), "r-1", server, session);

            RecordedRequest request = daemon.takeRequest();
            assertEquals(200, status);
            assertEquals("POST", request.getMethod());
            assertEquals("/api/v1/requests/r-1/decision", request.getPath());
            JSONObject body = new JSONObject(request.getBody().readUtf8());
            assertEquals(action[0], action[1], body.getString("decision"));
            assertFalse(action[0], body.has("scope"));
            assertEquals(1, body.length());
            assertEquals("pagis_session=s1", request.getHeader("Cookie"));
            assertEquals(1, request.getHeaders().values("Cookie").size());
            assertNull(request.getHeader("Origin"));
            assertNull(request.getHeader("Sec-Fetch-Site"));
            assertEquals("application/json; charset=utf-8", request.getHeader("Content-Type"));
        }
    }

    @Test
    public void theIdOfTheRequestIsOnePathSegment() throws Exception {
        daemon.enqueue(new MockResponse().setResponseCode(200));

        new ApprovalAnswer().post(ApprovalDecision.DENIED, "a/b c", server, session);

        assertEquals("/api/v1/requests/a%2Fb%20c/decision", daemon.takeRequest().getPath());
    }

    @Test
    public void aRefusalGivesItsStatus() throws Exception {
        daemon.enqueue(new MockResponse().setResponseCode(409));

        assertEquals(409, new ApprovalAnswer().post(ApprovalDecision.APPROVED, "r-1", server, session));
    }

    /** A redirect is not followed: the copy of the Session goes to the daemon alone. */
    @Test
    public void aRedirectIsNotFollowed() throws Exception {
        daemon.enqueue(new MockResponse().setResponseCode(302).setHeader("Location", "https://elsewhere.example/"));

        assertEquals(302, new ApprovalAnswer().post(ApprovalDecision.APPROVED, "r-1", server, session));
        assertEquals(1, daemon.getRequestCount());
    }

    /** The daemon keeps no cookie in the client: each post sends the copy alone. */
    @Test
    public void aCookieOfTheDaemonIsNotSentAgain() throws Exception {
        daemon.enqueue(new MockResponse().setResponseCode(200).addHeader("Set-Cookie", "other=1; Path=/"));
        daemon.enqueue(new MockResponse().setResponseCode(200));
        ApprovalAnswer answer = new ApprovalAnswer();

        answer.post(ApprovalDecision.APPROVED, "r-1", server, session);
        answer.post(ApprovalDecision.APPROVED, "r-2", server, session);

        daemon.takeRequest();
        assertEquals("pagis_session=s1", daemon.takeRequest().getHeader("Cookie"));
    }

    /**
     * Capacitor sets the cookie store of the web view as the default
     * {@link CookieHandler} of the process. The answer does not read it.
     */
    @Test
    public void theDefaultCookieHandlerOfTheProcessIsNotRead() throws Exception {
        CookieHandler before = CookieHandler.getDefault();
        CookieHandler.setDefault(new CookieHandler() {
            @Override
            public Map<String, List<String>> get(URI uri, Map<String, List<String>> headers) {
                return Collections.singletonMap("Cookie", Collections.singletonList("pagis_session=web"));
            }

            @Override
            public void put(URI uri, Map<String, List<String>> headers) {}
        });
        try {
            daemon.enqueue(new MockResponse().setResponseCode(200));

            new ApprovalAnswer().post(ApprovalDecision.APPROVED, "r-1", server, session);

            assertEquals(Collections.singletonList("pagis_session=s1"), daemon.takeRequest().getHeaders().values("Cookie"));
        } finally {
            CookieHandler.setDefault(before);
        }
    }

    @Test
    public void noDaemonIsAnIOException() throws Exception {
        ServerOrigin nobody = closedPort();

        assertThrows(IOException.class, () -> new ApprovalAnswer().post(ApprovalDecision.APPROVED, "r-1", nobody, session));
    }

    /** A loopback origin whose port takes no connection. */
    static ServerOrigin closedPort() throws IOException {
        int port;
        try (ServerSocket socket = new ServerSocket(0)) {
            port = socket.getLocalPort();
        }
        return ServerOrigin.parse("http://127.0.0.1:" + port, true);
    }

    @Test
    public void anActionOtherThanApproveOnceOrDenyIsNoDecision() {
        assertEquals(ApprovalDecision.APPROVED, ApprovalDecision.ofAction("approve_once"));
        assertEquals(ApprovalDecision.DENIED, ApprovalDecision.ofAction("deny"));
        assertNull(ApprovalDecision.ofAction("approve_always"));
        assertNull(ApprovalDecision.ofAction(null));
    }
}
